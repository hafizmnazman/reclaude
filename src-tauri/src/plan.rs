use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::logic::*;

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncodedRename {
    pub from: String,
    pub to: String,
    pub kind: String,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariantCount {
    pub label: String,
    pub pattern: String,
    pub count: usize,
}

pub struct Plan {
    pub old_path: PathBuf,
    pub new_path: PathBuf,
    pub old_name: String,
    pub new_name: String,
    pub case_only: bool,
    /// Every history folder belonging to this project. `from == to` is
    /// possible (e.g. "Foo Bar" → "Foo-Bar" encodes identically) — those
    /// need no rename but still get the deep fix.
    pub affected: Vec<EncodedRename>,
    /// The subset of `affected` that actually changes name, in execution order.
    pub encoded_renames: Vec<EncodedRename>,
    pub unverified: Vec<String>,
    pub claude_json_exists: bool,
    pub variant_counts: Vec<VariantCount>,
    pub total_matches: usize,
    pub deep_fix_files: usize,
    pub variants: Vec<Variant>,
}

enum ChildCheck {
    Child(String),
    NotChild,
    Unverified(String),
}

/// Build the full dry-run plan. Nothing on disk is touched here.
pub fn build_plan(old_path: &Path, new_name: &str) -> Result<Plan, String> {
    let parent = old_path
        .parent()
        .ok_or_else(|| "Can't rename a drive root.".to_string())?
        .to_path_buf();
    let old_name = old_path
        .file_name()
        .ok_or_else(|| "Can't rename a drive root.".to_string())?
        .to_string_lossy()
        .to_string();
    validate_new_name(&parent, &old_name, new_name)?;

    let new_path = parent.join(new_name);
    let old_s = path_str(old_path);
    let new_s = path_str(&new_path);
    let case_only = is_case_only(&old_name, new_name);

    let projects = projects_dir()?;
    let entries: Vec<String> = match fs::read_dir(&projects) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect(),
        Err(_) => Vec::new(),
    };
    let find_entry =
        |n: &str| entries.iter().find(|e| e.eq_ignore_ascii_case(n)).cloned();

    let old_encoded = encode_path(&old_s);
    let new_encoded = encode_path(&new_s);

    let mut affected: Vec<EncodedRename> = Vec::new();
    let mut from_set: HashSet<String> = HashSet::new();
    let mut unverified: Vec<String> = Vec::new();

    // 1. Exact match on the project's own encoded folder. find_entry is
    // case-insensitive, which also covers the uppercase-drive-letter variant.
    if let Some(actual) = find_entry(&old_encoded) {
        from_set.insert(actual.to_lowercase());
        affected.push(EncodedRename {
            from: actual,
            to: new_encoded.clone(),
            kind: "project history".into(),
        });
    }

    // 2. Nested projects: real keys in .claude.json that start with old path + "/".
    let cj = claude_json_path()?;
    let claude_json_exists = cj.is_file();
    let json_bytes: Option<Vec<u8>> = if claude_json_exists { fs::read(&cj).ok() } else { None };

    let old_fwd = old_s.replace('\\', "/");
    let new_fwd = new_s.replace('\\', "/");
    if let Some(bytes) = &json_bytes {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) {
            if let Some(obj) = v.get("projects").and_then(|x| x.as_object()) {
                for key in obj.keys() {
                    let prefix = match key.get(..old_fwd.len()) {
                        Some(p) => p,
                        None => continue,
                    };
                    if !prefix.eq_ignore_ascii_case(&old_fwd)
                        || key.as_bytes().get(old_fwd.len()) != Some(&b'/')
                    {
                        continue;
                    }
                    let enc_old = encode_path(key);
                    if let Some(actual) = find_entry(&enc_old) {
                        if from_set.contains(&actual.to_lowercase()) {
                            continue;
                        }
                        let suffix = &key[old_fwd.len()..];
                        let enc_new = encode_path(&format!("{}{}", new_fwd, suffix));
                        from_set.insert(actual.to_lowercase());
                        affected.push(EncodedRename {
                            from: actual,
                            to: enc_new,
                            kind: "nested project".into(),
                        });
                    }
                }
            }
        }
    }

    // 3. Other folders starting with oldEncoded + "-": the encoding is lossy
    // (a child "X\2" and a sibling "X 2" encode identically), so verify via
    // the cwd recorded in the folder's newest session file.
    let prefix_l = format!("{}-", old_encoded.to_lowercase());
    for name in &entries {
        let lname = name.to_lowercase();
        if !lname.starts_with(&prefix_l) || from_set.contains(&lname) {
            continue;
        }
        match verify_child_cwd(&projects.join(name), &old_s) {
            ChildCheck::Child(cwd) => {
                let suffix = &cwd[old_s.len()..];
                let enc_new = encode_path(&format!("{}{}", new_s, suffix));
                if *name != enc_new
                    && affected.iter().any(|r| r.to.eq_ignore_ascii_case(&enc_new))
                {
                    unverified.push(format!(
                        "{} — would collide with another renamed folder, skipped",
                        name
                    ));
                    continue;
                }
                from_set.insert(lname);
                affected.push(EncodedRename {
                    from: name.clone(),
                    to: enc_new,
                    kind: "verified child".into(),
                });
            }
            ChildCheck::NotChild => {} // a sibling — leave it alone, silently
            ChildCheck::Unverified(reason) => {
                unverified.push(format!("{} — {}, skipped", name, reason));
            }
        }
    }

    // Conflict checks. A target may not already exist on disk unless it's the
    // same folder (case-only change) or a folder that will itself be renamed
    // away first. No two targets may collide either.
    let moved_away: HashSet<String> = affected
        .iter()
        .filter(|r| !r.from.eq_ignore_ascii_case(&r.to))
        .map(|r| r.from.to_lowercase())
        .collect();
    let mut seen_targets: HashSet<String> = HashSet::new();
    for r in &affected {
        let to_l = r.to.to_lowercase();
        if !seen_targets.insert(to_l.clone()) {
            return Err(format!(
                "Two history folders would end up with the same name ({}). Aborting to avoid merging histories — rename the nested project separately first.",
                r.to
            ));
        }
        if r.from.eq_ignore_ascii_case(&r.to) {
            continue;
        }
        if let Some(existing) = entries.iter().find(|e| e.to_lowercase() == to_l) {
            if !existing.eq_ignore_ascii_case(&r.from) && !moved_away.contains(&to_l) {
                return Err(format!(
                    "A history folder named {} already exists in .claude\\projects. Renaming would merge two different histories, so this is blocked.",
                    r.to
                ));
            }
        }
    }

    // Children rename before parents so chains like A→A-x with an existing
    // child A-x→A-x-x can't collide mid-run.
    affected.sort_by(|a, b| b.from.len().cmp(&a.from.len()));
    let encoded_renames: Vec<EncodedRename> = affected
        .iter()
        .filter(|r| r.from != r.to)
        .cloned()
        .collect();

    // 4. Dry-run match counts in .claude.json, per path-spelling variant.
    let variants = make_variants(&old_s, &new_s);
    let mut variant_counts = Vec::new();
    let mut total_matches = 0usize;
    for v in &variants {
        let count = match &json_bytes {
            Some(bytes) => replace_bounded(bytes, v.old.as_bytes(), v.new.as_bytes()).1,
            None => 0,
        };
        total_matches += count;
        variant_counts.push(VariantCount {
            label: v.label.clone(),
            pattern: v.old.clone(),
            count,
        });
    }

    // 5. How many .jsonl files the deep fix would touch.
    let mut deep_fix_files = 0usize;
    for r in &affected {
        let mut files = Vec::new();
        collect_jsonl(&projects.join(&r.from), &mut files);
        deep_fix_files += files.len();
    }

    Ok(Plan {
        old_path: old_path.to_path_buf(),
        new_path,
        old_name,
        new_name: new_name.to_string(),
        case_only,
        affected,
        encoded_renames,
        unverified,
        claude_json_exists,
        variant_counts,
        total_matches,
        deep_fix_files,
        variants,
    })
}

/// Read the cwd recorded in the newest session file of an encoded folder and
/// decide whether it lives under `old_path`.
fn verify_child_cwd(folder: &Path, old_path: &str) -> ChildCheck {
    let newest = match newest_jsonl(folder) {
        Some(p) => p,
        None => return ChildCheck::Unverified("no session files to verify against".into()),
    };
    let file = match fs::File::open(&newest) {
        Ok(f) => f,
        Err(_) => return ChildCheck::Unverified("couldn't read its session file".into()),
    };
    let reader = std::io::BufReader::new(file);
    let stream = serde_json::Deserializer::from_reader(reader).into_iter::<serde_json::Value>();
    let mut cwd: Option<String> = None;
    for value in stream.take(25) {
        match value {
            Ok(v) => {
                if let Some(c) = v.get("cwd").and_then(|x| x.as_str()) {
                    cwd = Some(c.to_string());
                    break;
                }
            }
            Err(_) => return ChildCheck::Unverified("session file couldn't be parsed".into()),
        }
    }
    let cwd = match cwd {
        Some(c) => c.replace('/', "\\").trim_end_matches('\\').to_string(),
        None => return ChildCheck::Unverified("no cwd recorded in its session file".into()),
    };
    // ASCII-case-insensitive prefix check on raw bytes, so the suffix slice
    // at old_path.len() stays valid even for non-ASCII paths.
    let matches_prefix = cwd
        .get(..old_path.len())
        .map_or(false, |p| p.eq_ignore_ascii_case(old_path));
    if matches_prefix
        && (cwd.len() == old_path.len() || cwd.as_bytes().get(old_path.len()) == Some(&b'\\'))
    {
        ChildCheck::Child(cwd)
    } else {
        ChildCheck::NotChild
    }
}

fn newest_jsonl(folder: &Path) -> Option<PathBuf> {
    let rd = fs::read_dir(folder).ok()?;
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .map_or(false, |x| x.eq_ignore_ascii_case("jsonl"))
        })
        .max_by_key(|p| {
            fs::metadata(p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH)
        })
}
