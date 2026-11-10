//! Run folder files: ids, seeds, JSON written the way `tellfold/runner.py` writes it, and `status.json`.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local, Utc};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tellfold_models::{json, KnowledgeGraph, Persona, RunStatus};

pub const STATUS_FILE: &str = "status.json";
pub const SEEDS_DIR: &str = "seeds";
pub const CONFIG_FILE: &str = "config.json";
pub const GRAPH_FILE: &str = "graph.json";
pub const PERSONAS_FILE: &str = "personas.json";
pub const REPORT_FILE: &str = "report.json";
pub const REWRITE_FILE: &str = "rewrite.json";
pub const CALLS_FILE: &str = "calls.jsonl";
pub const WORLD_FILE: &str = "world.db";

/// A run cannot continue for a reason the user can fix (e.g. empty seed files).
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct RunError(pub String);

/// `YYYYMMDD-HHMMSS-<slug>` in local time, the slug made from the question.
pub fn make_run_id(question: &str, now: Option<DateTime<Local>>) -> String {
    let lower = question.to_lowercase();
    let mut slug = String::new();
    let mut in_gap = false;
    for c in lower.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
            in_gap = false;
        } else if !in_gap {
            slug.push('-');
            in_gap = true;
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(40).collect();
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "run" } else { slug };
    format!("{}-{slug}", now.unwrap_or_else(Local::now).format("%Y%m%d-%H%M%S"))
}

/// A new, empty run folder under `out`; `-2`, `-3`, ... is appended when the id is taken.
pub fn create_run_dir(out: &Path, question: &str) -> io::Result<PathBuf> {
    let base = make_run_id(question, None);
    let mut run_dir = out.join(&base);
    let mut n = 2;
    loop {
        fs::create_dir_all(out)?;
        match fs::create_dir(&run_dir) {
            Ok(()) => return Ok(run_dir),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                run_dir = out.join(format!("{base}-{n}"));
                n += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Copy the seed documents into `<run>/seeds/`, renaming `name-2.ext`, ... on a clash.
pub fn copy_seeds(run_dir: &Path, files: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    let dest = run_dir.join(SEEDS_DIR);
    match fs::create_dir(&dest) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let mut copies = Vec::new();
    for f in files {
        let name = f.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seed has no name"))?;
        let mut target = dest.join(name);
        let mut n = 2;
        while target.exists() {
            let stem = f.file_stem().unwrap_or_default().to_string_lossy();
            let suffix = f.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            target = dest.join(format!("{stem}-{n}{suffix}"));
            n += 1;
        }
        fs::copy(f, &target)?;
        copies.push(target);
    }
    Ok(copies)
}

/// `model_dump_json(indent=2)` plus a newline: the text of `config.json`, `report.json`, `status.json`, ...
pub fn json_text<T: Serialize + ?Sized>(value: &T) -> String {
    json::to_string_pretty(value) + "\n"
}

/// The text `runner.write_json` writes for a list: each item indented on its own, starting at column 0.
pub fn json_list_text<T: Serialize>(items: &[T]) -> String {
    let parts: Vec<String> = items.iter().map(json::to_string_pretty).collect();
    format!("[\n{}\n]\n", parts.join(",\n"))
}

pub fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> io::Result<()> {
    fs::write(path, json_text(value))
}

pub fn write_json_list<T: Serialize>(path: &Path, items: &[T]) -> io::Result<()> {
    fs::write(path, json_list_text(items))
}

/// Write `text` to `path` through a temporary file in the same folder and a rename, so readers never see a partial
/// file. The temporary file is `<prefix>XXXX.tmp`, mode 0600 like Python's `mkstemp`.
pub fn atomic_write(path: &Path, prefix: &str, text: &str) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no folder"))?;
    let mut tmp = tempfile::Builder::new().prefix(prefix).suffix(".tmp").tempfile_in(dir)?;
    tmp.write_all(text.as_bytes())?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// A JSON file as a model, or None when it is missing, unreadable or does not validate.
pub fn read_model<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_status(run_dir: &Path, status: &RunStatus) -> io::Result<()> {
    atomic_write(&run_dir.join(STATUS_FILE), ".status-", &json_text(status))
}

/// The run's status, or None when status.json is missing or unreadable (older runs).
pub fn read_status(run_dir: &Path) -> Option<RunStatus> {
    read_model(&run_dir.join(STATUS_FILE))
}

/// Now in UTC, ISO 8601 to the second: `2026-11-10T09:30:00Z`.
pub fn now_iso() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// `datetime.fromtimestamp(ts, UTC).isoformat(timespec="seconds")` with `Z`: microsecond rounding, then the
/// fraction dropped.
pub fn iso_from_timestamp(ts: f64) -> String {
    let micros = (ts * 1e6).round_ties_even() as i64;
    let secs = micros.div_euclid(1_000_000);
    DateTime::<Utc>::from_timestamp(secs, 0).unwrap_or_default().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// A file's modification time in seconds since the epoch, 0 when it cannot be read.
pub fn mtime(path: &Path) -> f64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn now_timestamp() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// The topic map and the people of a finished run, reused by a run with `cast_from` set.
#[derive(Debug, Clone)]
pub struct Cast {
    pub graph: KnowledgeGraph,
    pub personas: Vec<Persona>,
}

pub fn load_cast(source_dir: &Path) -> Result<Cast, RunError> {
    let name = source_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let graph: Option<KnowledgeGraph> = read_model(&source_dir.join(GRAPH_FILE));
    let personas: Option<Vec<Persona>> = read_model(&source_dir.join(PERSONAS_FILE));
    let (Some(graph), Some(personas)) = (graph, personas) else {
        return Err(RunError(format!("cannot reuse the people of {name}: it has no topic map or people yet")));
    };
    if personas.is_empty() {
        return Err(RunError(format!("cannot reuse the people of {name}: it has no people")));
    }
    Ok(Cast { graph, personas })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn run_ids() {
        let now = Local.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
        assert_eq!(
            make_run_id("Will buyers accept the subscription?", Some(now)),
            "20260102-030405-will-buyers-accept-the-subscription"
        );
        assert_eq!(make_run_id("¿Qué?", Some(now)), "20260102-030405-qu");
        assert_eq!(make_run_id("!!!", Some(now)), "20260102-030405-run");
        assert_eq!(
            make_run_id(&"word ".repeat(20), Some(now)),
            "20260102-030405-word-word-word-word-word-word-word-word"
        );
    }

    #[test]
    fn timestamps() {
        assert_eq!(iso_from_timestamp(0.0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_from_timestamp(1_700_000_000.9), "2023-11-14T22:13:20Z");
    }
}
