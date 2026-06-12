//! End-to-end pipeline test against a sandboxed fake %USERPROFILE% /
//! %LOCALAPPDATA%, covering the acceptance checklist items that can be
//! verified without a GUI: never-opened projects, the sibling-name trap,
//! nested projects, deep fix, undo byte-fidelity, and case-only renames.

use std::fs;
use std::path::Path;

use reclaude::exec;
use reclaude::logic::encode_path;
use reclaude::plan::build_plan;

fn write(p: &Path, content: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

fn esc(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "\\\\")
}

fn fwd_lower(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let mut s = s.to_string();
    if !s.is_empty() {
        let f = s.remove(0).to_ascii_lowercase();
        s.insert(0, f);
    }
    s
}

#[test]
fn full_pipeline() {
    // ---- sandbox ----
    let root = std::env::temp_dir().join(format!("reclaude-it-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let home = root.join("home");
    let lad = root.join("lad");
    let work = root.join("work");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&lad).unwrap();
    fs::create_dir_all(&work).unwrap();
    std::env::set_var("USERPROFILE", &home);
    std::env::set_var("LOCALAPPDATA", &lad);
    let projects = home.join(".claude").join("projects");
    fs::create_dir_all(&projects).unwrap();

    // ---- acceptance 1: a folder never opened in Claude ----
    let fresh = work.join("Fresh");
    fs::create_dir_all(&fresh).unwrap();
    let r = exec::execute(&fresh, "Fresher", true);
    assert!(r.ok, "{:?}", r.error);
    assert!(work.join("Fresher").is_dir());
    assert!(r.summary.iter().any(|s| s.contains("never opened")));

    // ---- main fixture ----
    let old = work.join("Final Year Project");
    let sibling = work.join("Final Year Project 2");
    fs::create_dir_all(old.join("sub")).unwrap();
    fs::create_dir_all(&sibling).unwrap();
    write(&old.join("code.txt"), "hello");

    let new = work.join("New Name");

    let enc_proj = encode_path(&old.to_string_lossy());
    let enc_sib = encode_path(&sibling.to_string_lossy());
    let enc_child = encode_path(&old.join("sub").to_string_lossy());
    let enc_tool = encode_path(&old.join("tool").to_string_lossy());
    assert_eq!(enc_sib, format!("{enc_proj}-2")); // the lossy-encoding trap

    // project history: first line has no cwd (like real summary lines)
    write(
        &projects.join(&enc_proj).join("session1.jsonl"),
        &format!(
            "{{\"type\":\"summary\",\"summary\":\"x\"}}\n{{\"cwd\":\"{}\",\"sessionId\":\"a\"}}\n",
            esc(&old)
        ),
    );
    // sibling history: cwd points at the sibling — must be left alone
    write(
        &projects.join(&enc_sib).join("sib.jsonl"),
        &format!("{{\"cwd\":\"{}\"}}\n", esc(&sibling)),
    );
    // nested project (also a key in .claude.json)
    write(
        &projects.join(&enc_child).join("child.jsonl"),
        &format!("{{\"cwd\":\"{}\"}}\n", esc(&old.join("sub"))),
    );
    // verified child that is NOT in .claude.json (found via prefix scan + cwd)
    write(
        &projects.join(&enc_tool).join("tool.jsonl"),
        &format!("{{\"cwd\":\"{}\"}}\n", esc(&old.join("tool"))),
    );
    // unverifiable folder: prefix matches but no session files
    fs::create_dir_all(projects.join(format!("{enc_proj}-mystery"))).unwrap();

    let cj = home.join(".claude.json");
    let cj_text = format!(
        "{{\n  \"numStartups\": 5,\n  \"projects\": {{\n    \"{old_f}\": {{\"history\": [1]}},\n    \"{old_f}/sub\": {{\"x\": 1}},\n    \"{sib_f}\": {{\"y\": 2}}\n  }},\n  \"other\": \"{old_e}\\\\notes.txt\"\n}}\n",
        old_f = fwd_lower(&old),
        sib_f = fwd_lower(&sibling),
        old_e = old.to_string_lossy().replace('\\', "\\\\"),
    );
    fs::write(&cj, &cj_text).unwrap();
    let cj_original = fs::read(&cj).unwrap();
    let sib_jsonl_original = fs::read(projects.join(&enc_sib).join("sib.jsonl")).unwrap();

    // ---- preview (acceptance 7: counts must match what execute reports) ----
    let plan = build_plan(&old, "New Name").unwrap();
    assert_eq!(plan.total_matches, 3, "2 fwd-lower (key + nested key) + 1 esc in 'other'");
    assert_eq!(plan.encoded_renames.len(), 3, "project, nested, verified child");
    assert_eq!(plan.unverified.len(), 1);
    assert!(plan.unverified[0].contains("mystery"));

    // ---- execute ----
    let r = exec::execute(&old, "New Name", true);
    assert!(r.ok, "{:?}", r.error);

    // disk folder
    assert!(new.join("code.txt").is_file());
    assert!(!old.exists());
    // sibling untouched on disk, in projects, and in its session file
    assert!(sibling.is_dir());
    assert!(projects.join(&enc_sib).is_dir());
    assert_eq!(
        fs::read(projects.join(&enc_sib).join("sib.jsonl")).unwrap(),
        sib_jsonl_original
    );
    // encoded folders renamed (acceptance 3, 4)
    let enc_new = encode_path(&new.to_string_lossy());
    let enc_new_child = encode_path(&new.join("sub").to_string_lossy());
    let enc_new_tool = encode_path(&new.join("tool").to_string_lossy());
    assert!(projects.join(&enc_new).is_dir());
    assert!(projects.join(&enc_new_child).is_dir());
    assert!(projects.join(&enc_new_tool).is_dir());
    assert!(!projects.join(&enc_proj).exists());
    assert!(projects.join(format!("{enc_proj}-mystery")).is_dir(), "unverified left alone");

    // .claude.json: valid JSON, no BOM, only intended keys changed
    let cj_after = fs::read(&cj).unwrap();
    assert_ne!(&cj_after[..3], &[0xEF, 0xBB, 0xBF], "must stay BOM-free");
    let v: serde_json::Value = serde_json::from_slice(&cj_after).unwrap();
    let proj_keys = v["projects"].as_object().unwrap();
    assert!(proj_keys.contains_key(&fwd_lower(&new)));
    assert!(proj_keys.contains_key(&format!("{}/sub", fwd_lower(&new))));
    assert!(proj_keys.contains_key(&fwd_lower(&sibling)), "sibling key untouched");
    assert!(!proj_keys.contains_key(&fwd_lower(&old)));
    assert_eq!(
        v["other"].as_str().unwrap(),
        format!("{}\\notes.txt", new.to_string_lossy())
    );

    // deep fix: session files now embed the new path
    let s1 = fs::read_to_string(projects.join(&enc_new).join("session1.jsonl")).unwrap();
    assert!(s1.contains(&esc(&new)));
    assert!(!s1.contains(&esc(&old)));
    let tool = fs::read_to_string(projects.join(&enc_new_tool).join("tool.jsonl")).unwrap();
    assert!(tool.contains(&esc(&new.join("tool"))));

    // ---- undo (acceptance 8): byte-for-byte restoration ----
    let info = exec::manifest_info().unwrap();
    assert!(!info.undone);
    let u = exec::undo().unwrap();
    assert!(!u.summary.is_empty());
    assert!(old.join("code.txt").is_file());
    assert!(!new.exists());
    assert!(projects.join(&enc_proj).is_dir());
    assert!(projects.join(&enc_child).is_dir());
    assert!(projects.join(&enc_tool).is_dir());
    assert_eq!(fs::read(&cj).unwrap(), cj_original, ".claude.json must match byte for byte");
    let s1 = fs::read_to_string(projects.join(&enc_proj).join("session1.jsonl")).unwrap();
    assert!(s1.contains(&esc(&old)));
    assert!(exec::manifest_info().unwrap().undone);
    assert!(exec::undo().is_err(), "double undo must refuse");

    // ---- acceptance 6: a mid-run failure rolls back automatically ----
    // Force step 4 (.claude.json edit) to fail after the folder renames by
    // planting a directory where write_swap wants to create its temp file.
    let blocker = home.join(".claude.json.reclaude-tmp");
    fs::create_dir_all(&blocker).unwrap();
    let r = exec::execute(&old, "New Name", true);
    fs::remove_dir_all(&blocker).unwrap();
    assert!(!r.ok, "execute must fail with the temp path blocked");
    assert!(r.rolled_back, "rollback must succeed: {:?}", r.error);
    assert!(old.is_dir(), "real folder restored");
    assert!(!new.exists());
    assert!(projects.join(&enc_proj).is_dir(), "encoded folder restored");
    assert!(projects.join(&enc_child).is_dir());
    assert!(!projects.join(&enc_new).exists());
    assert_eq!(
        fs::read(&cj).unwrap(),
        cj_original,
        ".claude.json untouched after rollback"
    );

    // ---- acceptance 9: case-only rename ----
    let case_dir = work.join("CaseProj");
    fs::create_dir_all(&case_dir).unwrap();
    let enc_case = encode_path(&case_dir.to_string_lossy());
    write(
        &projects.join(&enc_case).join("s.jsonl"),
        &format!("{{\"cwd\":\"{}\"}}\n", esc(&case_dir)),
    );
    let r = exec::execute(&case_dir, "caseproj", true);
    assert!(r.ok, "{:?}", r.error);
    let actual_name = fs::read_dir(&work)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .find(|n| n.eq_ignore_ascii_case("caseproj"))
        .unwrap();
    assert_eq!(actual_name, "caseproj", "on-disk casing must change");
    let enc_case_new = encode_path(&work.join("caseproj").to_string_lossy());
    assert!(projects.join(&enc_case_new).is_dir());
    let listed: Vec<String> = fs::read_dir(&projects)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(listed.contains(&enc_case_new), "encoded folder casing must change: {listed:?}");

    let _ = fs::remove_dir_all(&root);
}
