use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::logic::*;
use crate::plan::build_plan;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteResult {
    pub ok: bool,
    pub locked: bool,
    pub rolled_back: bool,
    pub error: Option<String>,
    pub summary: Vec<String>,
    pub new_path: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PathPair {
    pub from: String,
    pub to: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: u32,
    pub timestamp: String,
    pub stamp: String,
    pub old_path: String,
    pub new_path: String,
    pub case_only: bool,
    pub encoded_renames: Vec<PathPair>,
    pub claude_json_path: Option<String>,
    pub claude_json_backup: Option<String>,
    pub claude_json_edited: bool,
    pub jsonl_zip: Option<String>,
    pub jsonl_edited: Vec<String>,
    pub backup_dir: String,
    pub undone: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestInfo {
    pub timestamp: String,
    pub old_path: String,
    pub new_path: String,
    pub undone: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoResult {
    pub summary: Vec<String>,
    pub restored_path: String,
}

fn manifest_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("rename-manifest.json"))
}

pub fn read_manifest() -> Option<Manifest> {
    let p = manifest_path().ok()?;
    let bytes = fs::read(p).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn manifest_info() -> Option<ManifestInfo> {
    read_manifest().map(|m| ManifestInfo {
        timestamp: m.timestamp,
        old_path: m.old_path,
        new_path: m.new_path,
        undone: m.undone,
    })
}

fn is_lock_err(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(5) | Some(32) | Some(33))
}

fn lock_message(what: &str) -> String {
    format!(
        "{} is being used by another program. Close apps using this folder (VS Code, terminals, Explorer windows showing it) and click Retry.",
        what
    )
}

/// Rename a directory, going through a temp name when only the letter case
/// changes (Windows treats the two names as the same folder).
fn rename_dir(from: &Path, to: &Path, case_only: bool) -> io::Result<()> {
    if case_only {
        let tmp = from.with_file_name(format!(
            "{}.reclaude-tmp-{}",
            to.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id()
        ));
        fs::rename(from, &tmp)?;
        if let Err(e) = fs::rename(&tmp, to) {
            let _ = fs::rename(&tmp, from);
            return Err(e);
        }
        Ok(())
    } else {
        fs::rename(from, to)
    }
}

/// Replace a file's contents via a temp file in the same directory. A backup
/// always exists before this is called, so a crash mid-swap is recoverable.
fn write_swap(path: &Path, data: &[u8]) -> io::Result<()> {
    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!("{}.reclaude-tmp", file_name));
    fs::write(&tmp, data)?;
    if path.exists() {
        if let Err(e) = fs::remove_file(path) {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
    }
    if let Err(e) = fs::rename(&tmp, path) {
        // last resort: copy the temp back so the file isn't left missing
        fs::copy(&tmp, path)?;
        let _ = fs::remove_file(&tmp);
        let _ = e;
    }
    Ok(())
}

fn create_zip(zip_path: &Path, files: &[(String, PathBuf)]) -> Result<(), String> {
    let f = fs::File::create(zip_path).map_err(|e| format!("couldn't create backup zip: {e}"))?;
    let mut zw = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (entry, src) in files {
        zw.start_file(entry.replace('\\', "/"), opts)
            .map_err(|e| format!("backup zip error: {e}"))?;
        let mut r = fs::File::open(src).map_err(|e| format!("couldn't read {}: {e}", src.display()))?;
        io::copy(&mut r, &mut zw).map_err(|e| format!("backup zip error: {e}"))?;
    }
    zw.finish().map_err(|e| format!("backup zip error: {e}"))?;
    Ok(())
}

/// Extract every entry of the session backup zip back into the projects dir.
/// Entries are stored as "<original encoded folder name>/<relative path>".
fn restore_zip(zip_path: &Path, projects: &Path) -> Result<usize, String> {
    let f = fs::File::open(zip_path).map_err(|e| format!("couldn't open backup zip: {e}"))?;
    let mut za = zip::ZipArchive::new(f).map_err(|e| format!("couldn't read backup zip: {e}"))?;
    let mut restored = 0;
    for i in 0..za.len() {
        let mut zf = za.by_index(i).map_err(|e| format!("backup zip error: {e}"))?;
        let rel = zf
            .enclosed_name()
            .ok_or_else(|| "backup zip contains an invalid path".to_string())?;
        let dest = projects.join(rel);
        if let Some(dir) = dest.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let mut out = fs::File::create(&dest)
            .map_err(|e| format!("couldn't restore {}: {e}", dest.display()))?;
        io::copy(&mut zf, &mut out).map_err(|e| format!("couldn't restore {}: {e}", dest.display()))?;
        restored += 1;
    }
    Ok(restored)
}

fn prune_backups(backups_root: &Path, keep: usize) {
    let Ok(rd) = fs::read_dir(backups_root) else { return };
    let mut dirs: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    while dirs.len() > keep {
        let oldest = dirs.remove(0);
        let _ = fs::remove_dir_all(oldest);
    }
}

pub fn execute(old_path: &Path, new_name: &str, deep_fix: bool) -> ExecuteResult {
    fn fail(msg: String) -> ExecuteResult {
        ExecuteResult {
            ok: false,
            locked: false,
            rolled_back: false,
            error: Some(msg),
            summary: Vec::new(),
            new_path: None,
        }
    }

    let plan = match build_plan(old_path, new_name) {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    let projects = match projects_dir() {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    let cj = match claude_json_path() {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    let app_data = match app_data_dir() {
        Ok(p) => p,
        Err(e) => return fail(e),
    };

    let now = chrono::Local::now();
    let mut stamp = now.format("%Y%m%d-%H%M%S").to_string();
    let backups_root = app_data.join("backups");
    let mut backup_dir = backups_root.join(&stamp);
    let mut n = 2;
    while backup_dir.exists() {
        stamp = format!("{}-{}", now.format("%Y%m%d-%H%M%S"), n);
        backup_dir = backups_root.join(&stamp);
        n += 1;
    }
    if let Err(e) = fs::create_dir_all(&backup_dir) {
        return fail(format!("Couldn't create the backup folder: {e}"));
    }

    // ---- Step 1: backups (nothing is modified yet) ----
    let mut summary: Vec<String> = Vec::new();
    let mut cj_backup: Option<PathBuf> = None;
    if cj.is_file() {
        let b = backup_dir.join(format!(".claude.json.bak-{}", stamp));
        if let Err(e) = fs::copy(&cj, &b) {
            return fail(format!("Couldn't back up .claude.json: {e}. Nothing was changed."));
        }
        cj_backup = Some(b);
    }

    let mut zip_path: Option<PathBuf> = None;
    let mut zip_count = 0usize;
    if deep_fix && !plan.affected.is_empty() {
        let mut files: Vec<(String, PathBuf)> = Vec::new();
        for r in &plan.affected {
            let folder = projects.join(&r.from);
            let mut found = Vec::new();
            collect_jsonl(&folder, &mut found);
            for f in found {
                if let Ok(rel) = f.strip_prefix(&projects) {
                    files.push((rel.to_string_lossy().to_string(), f.clone()));
                }
            }
        }
        if !files.is_empty() {
            let zp = backup_dir.join(format!("sessions-{}.zip", stamp));
            if let Err(e) = create_zip(&zp, &files) {
                return fail(format!("Couldn't back up session files: {e}. Nothing was changed."));
            }
            zip_count = files.len();
            zip_path = Some(zp);
        }
    }
    prune_backups(&backups_root, 5);

    // ---- Step 2: rename the real folder ----
    if let Err(e) = rename_dir(&plan.old_path, &plan.new_path, plan.case_only) {
        let msg = if is_lock_err(&e) {
            lock_message("The folder")
        } else {
            format!("Couldn't rename the folder: {e}")
        };
        return ExecuteResult {
            ok: false,
            locked: is_lock_err(&e),
            rolled_back: true, // nothing was touched
            error: Some(msg),
            summary: Vec::new(),
            new_path: None,
        };
    }
    summary.push(format!(
        "Renamed folder: {} → {}",
        path_str(&plan.old_path),
        path_str(&plan.new_path)
    ));

    // Rollback bookkeeping from this point on.
    let mut done_encoded: Vec<(PathBuf, PathBuf)> = Vec::new(); // (from, to)
    let mut cj_edited = false;
    let mut jsonl_edited: Vec<PathBuf> = Vec::new();

    let rollback = |done_encoded: &[(PathBuf, PathBuf)],
                    cj_edited: bool,
                    jsonl_edited: &[PathBuf]|
     -> Vec<String> {
        let mut problems = Vec::new();
        if cj_edited {
            if let Some(b) = &cj_backup {
                if let Err(e) = fs::copy(b, &cj) {
                    problems.push(format!(".claude.json could not be restored: {e}"));
                }
            }
        }
        for (from, to) in done_encoded.iter().rev() {
            if let Err(e) = fs::rename(to, from) {
                problems.push(format!(
                    "history folder {} could not be renamed back: {e}",
                    to.display()
                ));
            }
        }
        if let Err(e) = rename_dir(&plan.new_path, &plan.old_path, plan.case_only) {
            problems.push(format!("the project folder could not be renamed back: {e}"));
        }
        if !jsonl_edited.is_empty() {
            if let Some(zp) = &zip_path {
                if let Err(e) = restore_zip(zp, &projects) {
                    problems.push(format!("session files could not be restored: {e}"));
                }
            }
        }
        problems
    };

    let fail_rolled_back = |step_err: String,
                            done_encoded: &[(PathBuf, PathBuf)],
                            cj_edited: bool,
                            jsonl_edited: &[PathBuf]|
     -> ExecuteResult {
        let problems = rollback(done_encoded, cj_edited, jsonl_edited);
        let (rolled_back, error) = if problems.is_empty() {
            (
                true,
                format!("{step_err} Everything was rolled back — the system is back to its original state."),
            )
        } else {
            (
                false,
                format!(
                    "{step_err} Rollback was attempted but hit problems: {}. Backups are in {}.",
                    problems.join("; "),
                    backup_dir.display()
                ),
            )
        };
        ExecuteResult {
            ok: false,
            locked: false,
            rolled_back,
            error: Some(error),
            summary: Vec::new(),
            new_path: None,
        }
    };

    // ---- Step 3: rename the encoded history folder(s) ----
    for r in &plan.encoded_renames {
        let from = projects.join(&r.from);
        let to = projects.join(&r.to);
        let case_only_enc = r.from.eq_ignore_ascii_case(&r.to);
        if let Err(e) = rename_dir(&from, &to, case_only_enc) {
            return fail_rolled_back(
                format!("Couldn't rename history folder {}: {e}.", r.from),
                &done_encoded,
                cj_edited,
                &jsonl_edited,
            );
        }
        done_encoded.push((from, to));
        summary.push(format!("Renamed history folder ({}): {} → {}", r.kind, r.from, r.to));
    }
    for r in &plan.affected {
        if r.from == r.to {
            summary.push(format!(
                "History folder {} keeps its name — the new path encodes to the same folder.",
                r.from
            ));
        }
    }
    if plan.affected.is_empty() {
        summary.push(
            "No history folder found under .claude\\projects — this project was never opened in Claude Code, so the history steps were skipped.".into(),
        );
    }

    // ---- Step 4: literal text replacement in .claude.json ----
    if cj.is_file() {
        match fs::read(&cj) {
            Ok(bytes) => {
                let mut buf = bytes;
                let mut total = 0usize;
                let mut per_variant: Vec<String> = Vec::new();
                for v in &plan.variants {
                    let (nb, c) = replace_bounded(&buf, v.old.as_bytes(), v.new.as_bytes());
                    buf = nb;
                    total += c;
                    if c > 0 {
                        per_variant.push(format!("{} × {}", c, v.label));
                    }
                }
                if total > 0 {
                    if let Err(e) = write_swap(&cj, &buf) {
                        return fail_rolled_back(
                            format!("Couldn't update .claude.json: {e}."),
                            &done_encoded,
                            cj_edited,
                            &jsonl_edited,
                        );
                    }
                    cj_edited = true;
                    summary.push(format!(
                        "Updated .claude.json: {} replacement{} ({})",
                        total,
                        if total == 1 { "" } else { "s" },
                        per_variant.join(", ")
                    ));
                } else {
                    summary.push("No references found in .claude.json — nothing to update there.".into());
                }
            }
            Err(e) => {
                return fail_rolled_back(
                    format!("Couldn't read .claude.json: {e}."),
                    &done_encoded,
                    cj_edited,
                    &jsonl_edited,
                );
            }
        }
    } else {
        summary.push(".claude.json not found — skipped.".into());
    }

    // ---- Step 5: deep fix inside all affected history folders (including
    // ones whose encoded name didn't change) ----
    if deep_fix {
        let mut fixed = 0usize;
        for r in &plan.affected {
            let mut files = Vec::new();
            collect_jsonl(&projects.join(&r.to), &mut files);
            for f in files {
                let bytes = match fs::read(&f) {
                    Ok(b) => b,
                    Err(e) => {
                        return fail_rolled_back(
                            format!("Couldn't read session file {}: {e}.", f.display()),
                            &done_encoded,
                            cj_edited,
                            &jsonl_edited,
                        );
                    }
                };
                let mut buf = bytes;
                let mut changed = 0usize;
                for v in &plan.variants {
                    let (nb, c) = replace_bounded(&buf, v.old.as_bytes(), v.new.as_bytes());
                    buf = nb;
                    changed += c;
                }
                if changed > 0 {
                    if let Err(e) = write_swap(&f, &buf) {
                        return fail_rolled_back(
                            format!("Couldn't update session file {}: {e}.", f.display()),
                            &done_encoded,
                            cj_edited,
                            &jsonl_edited,
                        );
                    }
                    jsonl_edited.push(f);
                    fixed += 1;
                }
            }
        }
        if !plan.affected.is_empty() {
            summary.push(format!(
                "Deep fix: updated the embedded path in {} of {} session file{}.",
                fixed,
                zip_count,
                if zip_count == 1 { "" } else { "s" }
            ));
        }
    } else if !plan.affected.is_empty() {
        summary.push("Deep fix was off — session files keep the old embedded path (resumed sessions may reference it).".into());
    }

    // ---- Manifest (for Undo) ----
    let manifest = Manifest {
        version: 1,
        timestamp: now.format("%Y-%m-%d %H:%M:%S").to_string(),
        stamp: stamp.clone(),
        old_path: path_str(&plan.old_path),
        new_path: path_str(&plan.new_path),
        case_only: plan.case_only,
        encoded_renames: done_encoded
            .iter()
            .map(|(f, t)| PathPair { from: path_str(f), to: path_str(t) })
            .collect(),
        claude_json_path: if cj.is_file() { Some(path_str(&cj)) } else { None },
        claude_json_backup: cj_backup.as_deref().map(path_str),
        claude_json_edited: cj_edited,
        jsonl_zip: zip_path.as_deref().map(path_str),
        jsonl_edited: jsonl_edited.iter().map(|p| path_str(p)).collect(),
        backup_dir: path_str(&backup_dir),
        undone: false,
    };
    match manifest_path().and_then(|mp| {
        serde_json::to_vec_pretty(&manifest)
            .map_err(|e| e.to_string())
            .and_then(|b| fs::write(&mp, b).map_err(|e| e.to_string()))
    }) {
        Ok(()) => summary.push(format!("Backups saved to {}", backup_dir.display())),
        Err(e) => summary.push(format!(
            "Warning: couldn't write the undo manifest ({e}) — Undo won't be available for this rename. Backups are in {}.",
            backup_dir.display()
        )),
    }

    ExecuteResult {
        ok: true,
        locked: false,
        rolled_back: false,
        error: None,
        summary,
        new_path: Some(path_str(&plan.new_path)),
    }
}

pub fn undo() -> Result<UndoResult, String> {
    let mpath = manifest_path()?;
    let manifest = read_manifest().ok_or("No rename to undo.")?;
    if manifest.undone {
        return Err("The last rename was already undone.".into());
    }

    let new_p = PathBuf::from(&manifest.new_path);
    let old_p = PathBuf::from(&manifest.old_path);
    if !new_p.is_dir() {
        return Err(format!(
            "Folder not found at {} — it may have been moved or renamed since. Nothing was changed.",
            manifest.new_path
        ));
    }
    if !manifest.case_only && old_p.exists() {
        return Err(format!(
            "{} already exists, so the old name can't be restored. Nothing was changed.",
            manifest.old_path
        ));
    }

    let mut summary = Vec::new();

    rename_dir(&new_p, &old_p, manifest.case_only).map_err(|e| {
        if is_lock_err(&e) {
            lock_message("The folder")
        } else {
            format!("Couldn't rename the folder back: {e}. Nothing was changed.")
        }
    })?;
    summary.push(format!("Renamed folder back: {} → {}", manifest.new_path, manifest.old_path));

    let mut problems: Vec<String> = Vec::new();
    for pair in manifest.encoded_renames.iter().rev() {
        let to = PathBuf::from(&pair.to);
        let from = PathBuf::from(&pair.from);
        if !to.exists() {
            problems.push(format!("{} was missing", pair.to));
            continue;
        }
        match fs::rename(&to, &from) {
            Ok(()) => summary.push(format!(
                "Renamed history folder back: {}",
                from.file_name().unwrap_or_default().to_string_lossy()
            )),
            Err(e) => problems.push(format!("couldn't rename {} back: {e}", pair.to)),
        }
    }

    if manifest.claude_json_edited {
        match (&manifest.claude_json_backup, &manifest.claude_json_path) {
            (Some(b), Some(live)) => match fs::copy(b, live) {
                Ok(_) => summary.push("Restored .claude.json from backup.".into()),
                Err(e) => problems.push(format!(".claude.json couldn't be restored: {e}")),
            },
            _ => problems.push(".claude.json backup path missing from manifest".into()),
        }
    }

    if let Some(zp) = &manifest.jsonl_zip {
        if !manifest.jsonl_edited.is_empty() {
            let projects = projects_dir()?;
            match restore_zip(Path::new(zp), &projects) {
                Ok(n) => summary.push(format!("Restored {} session file{} from backup.", n, if n == 1 { "" } else { "s" })),
                Err(e) => problems.push(format!("session files couldn't be restored: {e}")),
            }
        }
    }

    let mut done = manifest.clone();
    done.undone = true;
    if let Ok(b) = serde_json::to_vec_pretty(&done) {
        let _ = fs::write(&mpath, b);
    }

    if problems.is_empty() {
        Ok(UndoResult {
            summary,
            restored_path: manifest.old_path,
        })
    } else {
        Err(format!(
            "Undo finished with problems: {}. Completed steps: {}",
            problems.join("; "),
            summary.join(" · ")
        ))
    }
}
