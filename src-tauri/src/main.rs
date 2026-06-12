#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::path::Path;

use reclaude::logic::*;
use reclaude::{exec, plan};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryStats {
    session_count: usize,
    total_bytes: u64,
    total_size: String,
    last_modified: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Inspection {
    path: String,
    parent: String,
    name: String,
    encoded: String,
    encoded_existing: Option<String>,
    history: Option<HistoryStats>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Preview {
    valid: bool,
    error: Option<String>,
    old_path: String,
    new_path: String,
    old_name: String,
    new_name: String,
    case_only: bool,
    encoded_renames: Vec<plan::EncodedRename>,
    unverified: Vec<String>,
    claude_json_exists: bool,
    variant_counts: Vec<plan::VariantCount>,
    total_matches: usize,
    deep_fix_file_count: usize,
    history_found: bool,
}

#[tauri::command]
fn inspect_project(path: String) -> Result<Inspection, String> {
    let p = clean_picked_path(&path)?;
    let s = path_str(&p);
    let encoded = encode_path(&s);
    let projects = projects_dir()?;

    let mut encoded_existing: Option<String> = None;
    if let Ok(rd) = fs::read_dir(&projects) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.eq_ignore_ascii_case(&encoded) {
                    encoded_existing = Some(name);
                    break;
                }
            }
        }
    }

    let history = encoded_existing.as_ref().map(|name| {
        let folder = projects.join(name);
        let mut count = 0usize;
        let mut bytes = 0u64;
        let mut last: Option<std::time::SystemTime> = None;
        let mut files = Vec::new();
        collect_jsonl(&folder, &mut files);
        for f in &files {
            if let Ok(md) = fs::metadata(f) {
                count += 1;
                bytes += md.len();
                if let Ok(m) = md.modified() {
                    if last.map_or(true, |l| m > l) {
                        last = Some(m);
                    }
                }
            }
        }
        HistoryStats {
            session_count: count,
            total_bytes: bytes,
            total_size: human_size(bytes),
            last_modified: last.map(|t| {
                chrono::DateTime::<chrono::Local>::from(t)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            }),
        }
    });

    Ok(Inspection {
        parent: p.parent().map(path_str).unwrap_or_default(),
        name: p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        path: s,
        encoded,
        encoded_existing,
        history,
    })
}

/// Pre-flight warnings: a running Claude Code process (it rewrites
/// .claude.json on exit) and processes whose working directory is inside the
/// folder (likely to lock the rename).
#[tauri::command]
fn get_warnings(path: String) -> Vec<String> {
    let Ok(p) = clean_picked_path(&path) else {
        return Vec::new();
    };
    env_warnings(&p)
}

fn env_warnings(folder: &Path) -> Vec<String> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let mut warnings = Vec::new();
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_cwd(UpdateKind::Always),
    );

    let folder_l = folder.to_string_lossy().to_lowercase();
    let own_pid = std::process::id();
    let mut claude: Vec<String> = Vec::new();
    let mut lockers: Vec<String> = Vec::new();

    for (pid, proc_) in sys.processes() {
        if pid.as_u32() == own_pid {
            continue;
        }
        let name = proc_.name().to_string_lossy().to_string();
        let lname = name.to_lowercase();
        let cmd = proc_
            .cmd()
            .iter()
            .map(|c| c.to_string_lossy().to_lowercase())
            .collect::<Vec<_>>()
            .join(" ");
        let is_claude = lname == "claude.exe"
            || lname == "claude"
            || ((lname == "node.exe" || lname == "node") && cmd.contains("claude"));
        if is_claude {
            claude.push(format!("{} (PID {})", name, pid.as_u32()));
        }
        if let Some(cwd) = proc_.cwd() {
            let c = cwd.to_string_lossy().to_lowercase();
            if c == folder_l || c.starts_with(&format!("{}\\", folder_l)) {
                lockers.push(format!("{} (PID {})", name, pid.as_u32()));
            }
        }
    }

    fn list(mut v: Vec<String>) -> String {
        v.sort();
        v.dedup();
        if v.len() > 4 {
            let extra = v.len() - 4;
            v.truncate(4);
            format!("{} and {} more", v.join(", "), extra)
        } else {
            v.join(", ")
        }
    }

    if !claude.is_empty() {
        warnings.push(format!(
            "Claude Code appears to be running: {}. Close it before renaming — it rewrites .claude.json on exit and would overwrite the changes made here.",
            list(claude)
        ));
    }
    if !lockers.is_empty() {
        warnings.push(format!(
            "These programs have their working directory inside this folder and may lock it: {}. Close them before renaming.",
            list(lockers)
        ));
    }
    warnings
}

#[tauri::command]
fn preview_rename(path: String, new_name: String, deep_fix: bool) -> Result<Preview, String> {
    let _ = deep_fix; // the plan always computes deep-fix counts; the flag only matters on execute
    let p = clean_picked_path(&path)?;
    match plan::build_plan(&p, &new_name) {
        Ok(plan) => Ok(Preview {
            valid: true,
            error: None,
            old_path: path_str(&plan.old_path),
            new_path: path_str(&plan.new_path),
            old_name: plan.old_name,
            new_name: plan.new_name,
            case_only: plan.case_only,
            history_found: !plan.affected.is_empty(),
            encoded_renames: plan.affected,
            unverified: plan.unverified,
            claude_json_exists: plan.claude_json_exists,
            variant_counts: plan.variant_counts,
            total_matches: plan.total_matches,
            deep_fix_file_count: plan.deep_fix_files,
        }),
        Err(e) => Ok(Preview {
            valid: false,
            error: Some(e),
            old_path: path_str(&p),
            new_path: String::new(),
            old_name: p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            new_name,
            case_only: false,
            history_found: false,
            encoded_renames: Vec::new(),
            unverified: Vec::new(),
            claude_json_exists: false,
            variant_counts: Vec::new(),
            total_matches: 0,
            deep_fix_file_count: 0,
        }),
    }
}

#[tauri::command]
fn execute_rename(path: String, new_name: String, deep_fix: bool) -> Result<exec::ExecuteResult, String> {
    let p = clean_picked_path(&path)?;
    Ok(exec::execute(&p, &new_name, deep_fix))
}

#[tauri::command]
fn undo_last() -> Result<exec::UndoResult, String> {
    exec::undo()
}

#[tauri::command]
fn last_manifest() -> Option<exec::ManifestInfo> {
    exec::manifest_info()
}

#[tauri::command]
fn open_in_explorer(path: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(&path);
    if !p.exists() {
        return Err("That folder no longer exists.".into());
    }
    std::process::Command::new("explorer.exe")
        .arg(&p)
        .spawn()
        .map_err(|e| format!("Couldn't open Explorer: {e}"))?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            inspect_project,
            get_warnings,
            preview_rename,
            execute_rename,
            undo_last,
            last_manifest,
            open_in_explorer
        ])
        .run(tauri::generate_context!())
        .expect("error while running Reclaude");
}
