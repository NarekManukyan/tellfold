//! Ports of the helper tests in `tests/test_cli.py` and the command-line parts of `tests/test_rename_compat.py` and
//! `tests/test_runtimes.py`. The whole commands are checked against Python in `cli_runs.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

use tellfold_cli::commands::{expand_seeds, parse_event};
use tellfold_cli::llm::{fake_mode, make_llm};
use tellfold_models::SimConfig;

#[test]
fn parse_event_reads_round_and_text() {
    let event = parse_event("3: Breaking: price cut", 5).unwrap();
    assert_eq!((event.round, event.content.as_str()), (3, "Breaking: price cut"));
    assert_eq!(parse_event(" 2 :x", 5).unwrap().round, 2, "int() takes surrounding spaces");
}

#[test]
fn parse_event_rejects_bad_input() {
    for spec in ["no colon", "x:text", "3:", "9:too late", "0:too early", ":text", "3:   "] {
        assert!(parse_event(spec, 5).is_err(), "{spec}");
    }
    assert_eq!(parse_event("9:late", 5).unwrap_err().message, "Invalid value for --event: round 9 is outside 1..5");
    assert_eq!(
        parse_event("nope", 5).unwrap_err().message,
        "Invalid value for --event: expected ROUND:TEXT, got 'nope'"
    );
}

#[test]
fn expand_seeds_reads_supported_files_from_folders() {
    let tmp = tempfile::tempdir().unwrap();
    for name in ["b.md", "a.txt", "c.pdf", "skip.png", "D.MARKDOWN"] {
        std::fs::write(tmp.path().join(name), "x").unwrap();
    }
    let names: Vec<String> = expand_seeds(&[tmp.path().to_path_buf()])
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["D.MARKDOWN", "a.txt", "b.md", "c.pdf"]);
    let file = PathBuf::from("kept/as/given.png");
    assert_eq!(expand_seeds(std::slice::from_ref(&file)), [file], "files are kept even when unsupported");
}

/// Every environment change lives in this one test, so tests running in parallel never see each other's.
#[test]
fn environment_switches() {
    // the old SHOAL_ names still work, and the new name wins over the old one
    std::env::remove_var("TELLFOLD_FAKE_LLM");
    std::env::set_var("SHOAL_FAKE_LLM", "1");
    assert!(fake_mode());
    std::env::set_var("TELLFOLD_FAKE_LLM", "0");
    assert!(!fake_mode(), "the new name wins over the old one");
    std::env::set_var("TELLFOLD_FAKE_LLM", " Yes ");
    assert!(fake_mode());
    std::env::remove_var("SHOAL_FAKE_LLM");

    // a run's config chooses its runtimes, and an unknown one says where to look
    std::env::remove_var("TELLFOLD_FAKE_LLM");
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("TELLFOLD_HOME", tmp.path().join("home"));
    let run_dir = tmp.path().join("run");
    std::fs::create_dir(&run_dir).unwrap();
    let config = SimConfig { agent_runtime: "gone".into(), ..SimConfig::new("q", Vec::new()) };
    tellfold_store::files::write_json(&run_dir.join("config.json"), &config).unwrap();
    let error = make_llm(Some(&run_dir), 2, false, None).err().expect("an unknown runtime");
    assert!(error.0.ends_with(". Run `tellfold runtimes` to see what is available."), "{}", error.0);
    assert!(make_llm(None, 1, false, None).is_ok(), "the settings default (claude-code) exists");
    std::env::remove_var("TELLFOLD_HOME");
}

fn tellfold() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tellfold"));
    command.env("COLUMNS", "80");
    command
}

#[test]
fn version_is_the_mac_app_version() {
    let project =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../mac/project.yml")).unwrap();
    let app = project
        .lines()
        .find_map(|l| l.trim().strip_prefix("MARKETING_VERSION: "))
        .expect("MARKETING_VERSION")
        .trim_matches('"');
    assert_eq!(tellfold_cli::VERSION, app, "engine-rs/Cargo.toml and mac/project.yml disagree");
    let out = tellfold().arg("--version").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("tellfold {app}\n"));
}

#[test]
fn help_lists_every_command_and_no_arguments_is_help() {
    let out = tellfold().arg("--help").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    for command in ["run", "report", "chat", "ask", "agents", "doctor", "compare", "check", "branch", "runtimes"] {
        assert!(text.contains(&format!("│ {command} ")), "{command}");
    }
    let bare = tellfold().output().unwrap();
    assert_eq!(bare.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&bare.stdout), text);
}

#[test]
fn shoal_command_is_a_deprecated_alias() {
    let tmp = tempfile::tempdir().unwrap();
    let shoal = tmp.path().join("shoal");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_tellfold"), &shoal).unwrap();
    let out = Command::new(&shoal).arg("--version").env("COLUMNS", "100").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("tellfold {}\n", tellfold_cli::VERSION));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "`shoal` is now `tellfold`. The old name still works for now; please switch.\n"
    );
}
