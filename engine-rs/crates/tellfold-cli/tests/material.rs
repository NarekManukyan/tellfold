//! `tellfold material`: the preview of audience material on the command line (Rust-only).

use std::process::Command;

fn material(args: &[&str], cwd: &std::path::Path) -> (i32, String) {
    let done = Command::new(env!("CARGO_BIN_EXE_tellfold"))
        .arg("material")
        .args(args)
        .current_dir(cwd)
        .env("TELLFOLD_FAKE_LLM", "1")
        .env("COLUMNS", "200")
        .output()
        .expect("the tellfold binary");
    (done.status.code().unwrap_or(-1), String::from_utf8_lossy(&done.stdout).into_owned())
}

#[test]
fn agents_warns_when_there_are_fewer_items_than_people() {
    let tmp = tempfile::tempdir().unwrap();
    let reviews: String = (0..35).map(|i| format!("Review {i}: the new price is too much for us\n\n")).collect();
    std::fs::write(tmp.path().join("reviews.txt"), reviews).unwrap();
    let (code, out) = material(&["reviews:reviews.txt", "--agents", "50", "--json"], tmp.path());
    assert_eq!(code, 0, "{out}");
    let preview: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(preview["summary"]["items"], 35);
    let warnings = preview["warnings"].to_string();
    assert!(warnings.contains("35 items for 50 people"), "{warnings}");
    // without --agents there is nothing to compare with
    let (_, out) = material(&["reviews:reviews.txt", "--json"], tmp.path());
    assert!(!out.contains("people:"), "{out}");
}

#[test]
fn a_home_relative_path_is_expanded() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("notes.txt"), "The bus is always late in the morning.").unwrap();
    let done = Command::new(env!("CARGO_BIN_EXE_tellfold"))
        .args(["material", "notes:~/notes.txt", "--json"])
        .env("HOME", home.path())
        .env("TELLFOLD_FAKE_LLM", "1")
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&done.stdout);
    assert!(done.status.success(), "{out}{}", String::from_utf8_lossy(&done.stderr));
    assert!(out.contains("The bus is always late"), "{out}");
}
