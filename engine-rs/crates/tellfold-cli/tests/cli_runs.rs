//! The `tellfold` binary against the Python command: the session recorded by `golden/make_cli_runs.py` (runs,
//! reports, chat, ask, studies, check, branch, rewrite and their errors on the canned model) is replayed in a fresh
//! folder, and every exit code, stdout, stderr and written file must match after the same normalisation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Step {
    args: Vec<String>,
    stdin: Option<String>,
    env: BTreeMap<String, String>,
    exit: i32,
    stdout: String,
    stderr: String,
}

#[derive(Deserialize)]
struct Session {
    runs: usize,
    studies: usize,
    steps: Vec<Step>,
    files: BTreeMap<String, String>,
}

const RUN_FILES: [&str; 8] = [
    "config.json",
    "status.json",
    "graph.json",
    "personas.json",
    "report.json",
    "report.md",
    "rewrite.json",
    "rewrite.md",
];
const STUDY_FILES: [&str; 3] = ["study.json", "compare.json", "agreement.json"];

fn fixture() -> Session {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../golden/fixtures/cli_runs/session.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("fixture JSON")
}

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../examples/mossbook_pricing.md")
}

fn run_dirs(cwd: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(cwd.join("runs"))
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir() && e.file_name() != "studies")
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Study ids in the order the runs joined them, then any others by name.
fn study_ids(cwd: &Path, runs: &[String]) -> Vec<String> {
    let mut ordered: Vec<String> = Vec::new();
    for name in runs {
        let text = std::fs::read_to_string(cwd.join("runs").join(name).join("config.json")).expect("config");
        let config: Value = serde_json::from_str(&text).expect("config JSON");
        if let Some(id) = config["study_id"].as_str().filter(|s| !s.is_empty()) {
            if !ordered.iter().any(|o| o == id) {
                ordered.push(id.into());
            }
        }
    }
    let mut rest: Vec<String> = std::fs::read_dir(cwd.join("runs/studies"))
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| !ordered.contains(n))
                .collect()
        })
        .unwrap_or_default();
    rest.sort();
    ordered.extend(rest);
    ordered
}

fn normalise(text: &str, cwd: &Path, runs: &[String], studies: &[String]) -> String {
    let mut folders = vec![cwd.to_string_lossy().into_owned()];
    if let Ok(real) = cwd.canonicalize() {
        folders.push(real.to_string_lossy().into_owned());
    }
    folders.sort_by_key(|f| std::cmp::Reverse(f.len()));
    folders.dedup();
    let mut text = text.to_string();
    for folder in &folders {
        text = text.replace(folder.as_str(), "<cwd>");
    }
    // longest first: a run made in the same second as another is named like it with "-2" added
    let mut names: Vec<(&str, String)> =
        runs.iter().enumerate().map(|(i, n)| (n.as_str(), format!("{{run:{i}}}"))).collect();
    names.extend(studies.iter().enumerate().map(|(i, n)| (n.as_str(), format!("{{study:{i}}}"))));
    names.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
    for (name, placeholder) in names {
        text = text.replace(name, &placeholder);
    }
    let timing = Regex::new(r"\b\d+\.\ds\b").unwrap();
    let stamp = Regex::new(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)?").unwrap();
    let pid = Regex::new(r#""pid": \d+"#).unwrap();
    let text = stamp.replace_all(&timing.replace_all(&text, "<t>s"), "<time>").into_owned();
    pid.replace_all(&text, r#""pid": 0"#).into_owned()
}

fn setup(cwd: &Path) {
    std::fs::copy(example(), cwd.join("seed.md")).expect("seed");
    std::fs::create_dir(cwd.join("empty")).unwrap();
    let seeds = cwd.join("seeds-dir");
    std::fs::create_dir(&seeds).unwrap();
    std::fs::write(seeds.join("b.md"), "Second note: the price goes up next month.\n").unwrap();
    std::fs::write(seeds.join("a.txt"), "First note: buyers liked the one-time price.\n").unwrap();
    std::fs::write(seeds.join("skip.png"), b"not a seed").unwrap();
}

fn listing(dir: &Path) -> String {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| entries.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    names.sort();
    names.join("\n")
}

#[test]
fn a_session_of_commands_matches_python() {
    let session = fixture();
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("work");
    std::fs::create_dir(&cwd).unwrap();
    setup(&cwd);
    let home = tmp.path().join("home");
    let mut made: Vec<String> = Vec::new();
    let mut outputs: Vec<(i32, String, String)> = Vec::new();
    let placeholder = Regex::new(r"\{run:(\d+)\}").unwrap();
    for step in &session.steps {
        let args: Vec<String> = step
            .args
            .iter()
            .map(|a| {
                placeholder
                    .replace_all(a, |c: &regex::Captures| made[c[1].parse::<usize>().unwrap()].clone())
                    .into_owned()
            })
            .collect();
        let mut command = Command::new(env!("CARGO_BIN_EXE_tellfold"));
        command.args(&args).current_dir(&cwd);
        for (key, _) in std::env::vars() {
            if key.starts_with("TELLFOLD_") || key.starts_with("SHOAL_") {
                command.env_remove(key);
            }
        }
        command.env("TELLFOLD_FAKE_LLM", "1").env("TELLFOLD_HOME", &home).env("COLUMNS", "100").env("LINES", "40");
        command.envs(&step.env);
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().expect("the tellfold binary");
        {
            use std::io::Write;
            let mut stdin = child.stdin.take().unwrap();
            if let Some(text) = &step.stdin {
                stdin.write_all(text.as_bytes()).unwrap();
            }
        }
        let done = child.wait_with_output().unwrap();
        for name in run_dirs(&cwd) {
            if !made.contains(&name) {
                made.push(name);
            }
        }
        outputs.push((
            done.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&done.stdout).into_owned(),
            String::from_utf8_lossy(&done.stderr).into_owned(),
        ));
    }
    let runs = made; // in the order the steps made them
    let mut sorted = runs.clone();
    sorted.sort();
    assert_eq!(sorted, run_dirs(&cwd));
    assert_eq!(runs.len(), session.runs);
    let studies = study_ids(&cwd, &runs);
    assert_eq!(studies.len(), session.studies);

    for (step, (exit, stdout, stderr)) in session.steps.iter().zip(&outputs) {
        let label = step.args.join(" ");
        let stdout = normalise(stdout, &cwd, &runs, &studies);
        let stderr = normalise(stderr, &cwd, &runs, &studies);
        assert_eq!(*exit, step.exit, "exit code of `{label}`\nstdout:\n{stdout}\nstderr:\n{stderr}");
        if step.args == ["doctor"] {
            // The Rust engine has no Python SDK to import: its doctor shows an engine row instead (see doctor()).
            for line in ["Claude Code CLI on PATH │ FAIL", "Logged in (haiku call)  │ ok", "tellfold doctor"] {
                assert!(stdout.contains(line), "doctor shows {line:?}:\n{stdout}");
            }
            continue;
        }
        assert_eq!(stdout, step.stdout, "stdout of `{label}`");
        assert_eq!(stderr, step.stderr, "stderr of `{label}`");
    }

    let mut files: BTreeMap<String, String> = BTreeMap::new();
    for (i, name) in runs.iter().enumerate() {
        for file in RUN_FILES {
            let path = cwd.join("runs").join(name).join(file);
            if path.is_file() {
                let text = std::fs::read_to_string(path).unwrap();
                files.insert(format!("runs/{{run:{i}}}/{file}"), normalise(&text, &cwd, &runs, &studies));
            }
        }
        files.insert(format!("runs/{{run:{i}}}/seeds/"), listing(&cwd.join("runs").join(name).join("seeds")));
    }
    for (i, id) in studies.iter().enumerate() {
        for file in STUDY_FILES {
            let path = cwd.join("runs/studies").join(id).join(file);
            if path.is_file() {
                let text = std::fs::read_to_string(path).unwrap();
                files.insert(format!("runs/studies/{{study:{i}}}/{file}"), normalise(&text, &cwd, &runs, &studies));
            }
        }
    }
    let elsewhere = cwd.join("elsewhere");
    files.insert("elsewhere/".into(), if elsewhere.is_dir() { listing(&elsewhere) } else { "<missing>".into() });
    assert_eq!(files.keys().collect::<Vec<_>>(), session.files.keys().collect::<Vec<_>>());
    for (name, want) in &session.files {
        assert_eq!(&files[name], want, "{name}");
    }
}
