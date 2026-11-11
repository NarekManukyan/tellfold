//! Turn a finished run into a built-in example: a self-contained, read-only copy the Mac app ships in its bundle
//! (port of `tellfold/examples.py`).
//!
//! The copy keeps what the app reads (config, graph, personas, world, report, status, calls log, seeds) and drops
//! what it must not ship or cannot open read-only: SQLite's WAL side files (the database is switched to a rollback
//! journal so `mode=ro&immutable=1` can open it from a signed bundle), the owning process id, and absolute seed
//! paths from the machine that made the run (in `config.json` and the footer of `report.html`).

use std::path::Path;

use serde_json::Value;
use tellfold_models::{Report, RunState, RunStatus, SimConfig};
use tellfold_report::html::render_html;
use tellfold_store::files::{write_json, write_status, SEEDS_DIR};
use tellfold_store::world::World;

use crate::branch::{copy_file, copy_tree, open_source};
use crate::Error;

pub const KEEP: [&str; 9] = [
    "config.json",
    "graph.json",
    "personas.json",
    "world.db",
    "calls.jsonl",
    "status.json",
    "report.json",
    "report.md",
    "report.html",
];
pub const PROMPT_KEYS: [&str; 5] = ["prompt", "system", "response", "reply", "content"];

fn parse<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, Error> {
    serde_json::from_str(&std::fs::read_to_string(path)?).map_err(|e| Error::Invalid(e.to_string()))
}

/// Copies finished run `run_dir` to the new folder `dest`, ready to ship. Returns `dest`.
pub fn pack_example(run_dir: &Path, dest: &Path) -> Result<std::path::PathBuf, Error> {
    let status: RunStatus = parse(&run_dir.join("status.json"))?;
    if status.state != RunState::Done || !run_dir.join("report.json").is_file() {
        return Err(Error::Example(format!("{} is not a finished run with a report", run_dir.display())));
    }
    if dest.exists() {
        return Err(Error::Example(format!("{} already exists", dest.display())));
    }
    std::fs::create_dir_all(dest)?;
    for name in KEEP {
        if run_dir.join(name).is_file() {
            copy_file(&run_dir.join(name), &dest.join(name))?;
        }
    }
    check_calls(&dest.join("calls.jsonl"))?;
    let seeds = dest.join(SEEDS_DIR);
    if seeds.exists() {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, seeds.display().to_string()).into());
    }
    copy_tree(&run_dir.join(SEEDS_DIR), &seeds)?;

    let src = open_source(&run_dir.join("world.db"), false)?;
    std::fs::remove_file(dest.join("world.db"))?;
    src.backup(rusqlite::DatabaseName::Main, dest.join("world.db"), None)?;
    drop(src);
    let out = rusqlite::Connection::open(dest.join("world.db"))?;
    out.query_row("PRAGMA journal_mode=DELETE", [], |_| Ok(()))?;
    out.execute_batch("VACUUM")?;
    out.close().map_err(|(_, e)| e)?;

    let mut config: SimConfig = parse(&dest.join("config.json"))?;
    let mut names: Vec<String> = std::fs::read_dir(&seeds)?
        .filter_map(Result::ok)
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    config.seed_files = names.iter().map(|n| format!("{SEEDS_DIR}/{n}")).collect();
    write_json(&dest.join("config.json"), &config)?;
    // report.html names the seed files: render it again so it shows the relative paths
    let report: Report = parse(&dest.join("report.json"))?;
    let world = World::open_read_only(&dest.join("world.db"), config.seed)?;
    let html = render_html(&report, &world, &config);
    world.close()?;
    std::fs::write(dest.join("report.html"), html?)?;
    write_status(dest, &RunStatus { pid: None, detail: String::new(), ..status })?;
    Ok(dest.to_path_buf())
}

/// The calls log must hold timings and outcomes only, never prompt or answer text.
fn check_calls(path: &Path) -> Result<(), Error> {
    if !path.is_file() {
        return Ok(());
    }
    for line in std::fs::read_to_string(path)?.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line).map_err(|e| Error::Invalid(e.to_string()))?;
        let has_text = match &value {
            Value::Object(map) => PROMPT_KEYS.iter().any(|k| map.contains_key(*k)),
            Value::Array(items) => items.iter().any(|v| v.as_str().is_some_and(|s| PROMPT_KEYS.contains(&s))),
            Value::String(s) => PROMPT_KEYS.iter().any(|k| s.contains(k)),
            _ => false,
        };
        if has_text {
            return Err(Error::Example(format!("{} holds prompt or answer text; it cannot ship", path.display())));
        }
    }
    Ok(())
}
