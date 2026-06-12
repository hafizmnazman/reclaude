use std::path::{Path, PathBuf};

/// Encode an absolute path into Claude Code's project-folder name:
/// every char that is not [A-Za-z0-9] becomes a dash (dashes are NOT
/// collapsed), then the first character (the drive letter) is lowercased.
pub fn encode_path(path: &str) -> String {
    let mut s: String = path
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    if !s.is_empty() {
        let first = s.remove(0).to_ascii_lowercase();
        s.insert(0, first);
    }
    s
}

pub fn upper_first(s: &str) -> String {
    let mut out = s.to_string();
    if !out.is_empty() {
        let first = out.remove(0).to_ascii_uppercase();
        out.insert(0, first);
    }
    out
}

fn lower_first(s: &str) -> String {
    let mut out = s.to_string();
    if !out.is_empty() {
        let first = out.remove(0).to_ascii_lowercase();
        out.insert(0, first);
    }
    out
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Variant {
    pub label: String,
    pub old: String,
    pub new: String,
}

/// The four plausible spellings of a Windows path inside .claude.json and
/// session .jsonl files. Old and new are styled identically per variant.
pub fn make_variants(old_path: &str, new_path: &str) -> Vec<Variant> {
    let fwd_old = old_path.replace('\\', "/");
    let fwd_new = new_path.replace('\\', "/");
    let esc_old = old_path.replace('\\', "\\\\");
    let esc_new = new_path.replace('\\', "\\\\");
    let raw = [
        ("forward slashes, lowercase drive (c:/…)", lower_first(&fwd_old), lower_first(&fwd_new)),
        ("forward slashes, uppercase drive (C:/…)", upper_first(&fwd_old), upper_first(&fwd_new)),
        ("escaped backslashes, lowercase drive (c:\\\\…)", lower_first(&esc_old), lower_first(&esc_new)),
        ("escaped backslashes, uppercase drive (C:\\\\…)", upper_first(&esc_old), upper_first(&esc_new)),
    ];
    let mut out: Vec<Variant> = Vec::new();
    for (label, old, new) in raw {
        if out.iter().any(|v| v.old == old) {
            continue; // e.g. UNC paths where drive-letter casing doesn't apply
        }
        out.push(Variant { label: label.to_string(), old, new });
    }
    out
}

/// Replace `needle` with `replacement`, but only where the byte right after
/// the match is `"` (end of a JSON string), `/` or `\` (start of a child
/// segment). This is what protects sibling folders like "X 2" when "X" is
/// renamed. Returns the new buffer and the number of replacements.
pub fn replace_bounded(buf: &[u8], needle: &[u8], replacement: &[u8]) -> (Vec<u8>, usize) {
    if needle.is_empty() || buf.len() < needle.len() {
        return (buf.to_vec(), 0);
    }
    let mut out = Vec::with_capacity(buf.len());
    let mut count = 0usize;
    let mut i = 0usize;
    while i < buf.len() {
        if buf.len() - i > needle.len() && &buf[i..i + needle.len()] == needle {
            let next = buf[i + needle.len()];
            if next == b'"' || next == b'/' || next == b'\\' {
                out.extend_from_slice(replacement);
                i += needle.len();
                count += 1;
                continue;
            }
        }
        out.push(buf[i]);
        i += 1;
    }
    (out, count)
}

/// Validate a proposed new folder name against Windows rules and collisions.
pub fn validate_new_name(parent: &Path, old_name: &str, new_name: &str) -> Result<(), String> {
    if new_name.is_empty() {
        return Err("Enter a new name.".into());
    }
    if new_name == old_name {
        return Err("That's already the folder's name.".into());
    }
    if new_name.starts_with(' ') || new_name.ends_with(' ') {
        return Err("Names can't start or end with a space.".into());
    }
    if new_name.ends_with('.') {
        return Err("Windows doesn't allow names ending with a dot.".into());
    }
    const INVALID: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    if new_name.chars().any(|c| INVALID.contains(&c) || (c as u32) < 0x20) {
        return Err("These characters aren't allowed: < > : \" / \\ | ? *".into());
    }
    let base = new_name.split('.').next().unwrap_or(new_name);
    let upper = base.trim().to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.as_bytes()[3].is_ascii_digit()
            && upper.as_bytes()[3] != b'0');
    if reserved {
        return Err(format!("\"{}\" is a reserved Windows name.", new_name));
    }
    if new_name.chars().count() > 255 {
        return Err("That name is too long.".into());
    }
    if !is_case_only(old_name, new_name) && parent.join(new_name).exists() {
        return Err(format!("\"{}\" already exists in this folder.", new_name));
    }
    Ok(())
}

/// True when the rename only changes letter case (needs a two-step rename
/// because Windows treats the names as the same folder).
pub fn is_case_only(old_name: &str, new_name: &str) -> bool {
    old_name != new_name && old_name.to_lowercase() == new_name.to_lowercase()
}

/// Normalize a picked path: trim whitespace/quotes, strip a verbatim prefix
/// and trailing slashes, and make sure it's an absolute existing directory.
pub fn clean_picked_path(raw: &str) -> Result<PathBuf, String> {
    let mut s = raw.trim().trim_matches('"').trim().to_string();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        s = stripped.to_string();
    }
    s = s.replace('/', "\\");
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    let p = PathBuf::from(&s);
    if !p.is_absolute() {
        return Err("Please pick a full folder path (e.g. C:\\Users\\you\\Projects\\MyApp).".into());
    }
    let md = std::fs::metadata(&p)
        .map_err(|_| "That folder doesn't exist or can't be read.".to_string())?;
    if !md.is_dir() {
        return Err("That's a file — pick a folder.".into());
    }
    if p.file_name().is_none() || p.parent().map_or(true, |q| q.as_os_str().is_empty()) {
        return Err("Can't rename a drive root.".into());
    }
    Ok(p)
}

pub fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

pub fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .ok_or_else(|| "USERPROFILE environment variable is not set.".into())
}

pub fn projects_dir() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".claude").join("projects"))
}

pub fn claude_json_path() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".claude.json"))
}

pub fn app_data_dir() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(|d| PathBuf::from(d).join("Reclaude"))
        .ok_or_else(|| "LOCALAPPDATA environment variable is not set.".into())
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{} {}", bytes, UNITS[u])
    } else {
        format!("{:.1} {}", v, UNITS[u])
    }
}

/// Recursively collect every .jsonl file under `dir`.
pub fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_jsonl(&p, out);
            } else if p
                .extension()
                .map_or(false, |x| x.eq_ignore_ascii_case("jsonl"))
            {
                out.push(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_spec_example() {
        assert_eq!(
            encode_path(r"C:\Users\abdul\OneDrive\Documents\Coding Projects\Final Year Project"),
            "c--Users-abdul-OneDrive-Documents-Coding-Projects-Final-Year-Project"
        );
    }

    #[test]
    fn encoding_does_not_collapse_dashes() {
        assert_eq!(encode_path(r"C:\a  b"), "c--a--b");
        assert_eq!(encode_path(r"D:\x-y"), "d--x-y");
    }

    #[test]
    fn replace_protects_siblings() {
        let json = br#"{"c:/proj/Final Year Project":1,"c:/proj/Final Year Project 2":2,"c:/proj/Final Year Project/sub":3}"#;
        let (out, n) = replace_bounded(json, b"c:/proj/Final Year Project", b"c:/proj/New Name");
        assert_eq!(n, 2); // the project itself and its child, not the sibling
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains(r#""c:/proj/New Name":1"#));
        assert!(s.contains(r#""c:/proj/Final Year Project 2":2"#));
        assert!(s.contains(r#""c:/proj/New Name/sub":3"#));
    }

    #[test]
    fn replace_handles_escaped_backslashes() {
        let json = br#"{"cwd":"C:\\proj\\Final Year Project","other":"C:\\proj\\Final Year Project 2"}"#;
        let (out, n) =
            replace_bounded(json, br"C:\\proj\\Final Year Project", br"C:\\proj\\New Name");
        assert_eq!(n, 1);
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains(r#""cwd":"C:\\proj\\New Name""#));
        assert!(s.contains(r"Final Year Project 2"));
    }

    #[test]
    fn replace_matches_child_with_backslash() {
        let json = br#"{"a":"C:\\proj\\X\\child"}"#;
        let (out, n) = replace_bounded(json, br"C:\\proj\\X", br"C:\\proj\\Y");
        assert_eq!(n, 1);
        assert!(String::from_utf8(out).unwrap().contains(r"C:\\proj\\Y\\child"));
    }

    #[test]
    fn four_variants_styled_correctly() {
        let v = make_variants(r"C:\proj\Old", r"C:\proj\New");
        assert_eq!(v.len(), 4);
        assert_eq!(v[0].old, "c:/proj/Old");
        assert_eq!(v[1].old, "C:/proj/Old");
        assert_eq!(v[2].old, r"c:\\proj\\Old");
        assert_eq!(v[3].old, r"C:\\proj\\Old");
        assert_eq!(v[2].new, r"c:\\proj\\New");
    }

    #[test]
    fn validates_windows_names() {
        let parent = std::env::temp_dir();
        assert!(validate_new_name(&parent, "Old", "").is_err());
        assert!(validate_new_name(&parent, "Old", "Old").is_err());
        assert!(validate_new_name(&parent, "Old", "bad:name").is_err());
        assert!(validate_new_name(&parent, "Old", "CON").is_err());
        assert!(validate_new_name(&parent, "Old", "com3").is_err());
        assert!(validate_new_name(&parent, "Old", "ends.").is_err());
        assert!(validate_new_name(&parent, "Old", "ends ").is_err());
        assert!(validate_new_name(&parent, "Old", " starts").is_err());
        assert!(validate_new_name(&parent, "Old", "fine-name_2026").is_ok());
        // case-only rename is allowed even though the target "exists"
        assert!(validate_new_name(&parent, "Old", "OLD").is_ok());
    }

    #[test]
    fn case_only_detection() {
        assert!(is_case_only("project", "Project"));
        assert!(!is_case_only("project", "project"));
        assert!(!is_case_only("project", "other"));
    }
}
