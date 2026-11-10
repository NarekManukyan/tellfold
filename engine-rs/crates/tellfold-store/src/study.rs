//! Studies: groups of runs that are compared with each other, kept in `<runs>/studies/<id>/` (port of
//! `tellfold/study.py`).
//!
//! `study.json` lists the member runs and their labels. A run made from another one ("Try another version…",
//! `--cast-from`) joins the source's study, or starts one with the source labelled "Draft A". The source can be a
//! read-only example: it joins through `study.json` only, never by changing its own files.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::{DateTime, Local};
use rand::Rng;
use regex::Regex;
use tellfold_models::text::{py_prefix, py_strip};
use tellfold_models::{SimConfig, Study, StudyKind, StudyRun};

use crate::files::{atomic_write, json_text, read_model};

pub const STUDIES_DIR: &str = "studies";
pub const STUDY_FILE: &str = "study.json";
pub const COMPARE_FILE: &str = "compare.json";
/// What held across the runs of a repeat study ("Check it").
pub const AGREEMENT_FILE: &str = "agreement.json";
const LETTERS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// The study cannot be read or joined for a reason the user can fix (bad id, unknown study).
#[derive(Debug, thiserror::Error)]
pub enum StudyError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

fn study_id_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,99}$").expect("valid pattern"))
}

/// The citation prefix of the study's index-th run: A, B, ... Z, then R27, R28, ...
pub fn letter(index: i64) -> String {
    if (0..26).contains(&index) {
        (LETTERS[index as usize] as char).to_string()
    } else {
        format!("R{}", index + 1)
    }
}

pub fn default_variant(index: i64) -> String {
    format!("Draft {}", letter(index))
}

/// `YYYYMMDD-HHMMSS-xxxx` (local time and four random hex digits).
pub fn new_study_id(now: Option<DateTime<Local>>) -> String {
    let suffix: u16 = rand::thread_rng().gen();
    format!("{}-{suffix:04x}", now.unwrap_or_else(Local::now).format("%Y%m%d-%H%M%S"))
}

pub fn study_dir(runs_dir: &Path, study_id: &str) -> Result<PathBuf, StudyError> {
    if !study_id_pattern().is_match(study_id) || study_id.contains("..") {
        return Err(StudyError::Invalid("invalid study id".into()));
    }
    Ok(runs_dir.join(STUDIES_DIR).join(study_id))
}

pub fn load_study(runs_dir: &Path, study_id: &str) -> Option<Study> {
    read_model(&study_dir(runs_dir, study_id).ok()?.join(STUDY_FILE))
}

/// Atomic write (temp file + rename), like status.json.
pub fn save_study(runs_dir: &Path, study: &Study) -> Result<(), StudyError> {
    let folder = study_dir(runs_dir, &study.id)?;
    fs::create_dir_all(&folder)?;
    atomic_write(&folder.join(STUDY_FILE), ".study-", &json_text(study))?;
    Ok(())
}

pub fn list_studies(runs_dir: &Path) -> Vec<Study> {
    let Ok(entries) = fs::read_dir(runs_dir.join(STUDIES_DIR)) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names.iter().filter_map(|name| load_study(runs_dir, name)).collect()
}

/// The study a run belongs to: its own `study_id`, else the first study that lists it (a source run).
pub fn study_of(runs_dir: &Path, run_id: &str, config: &SimConfig) -> Option<Study> {
    if let Some(study) = config.study_id.as_deref().and_then(|id| load_study(runs_dir, id)) {
        return Some(study);
    }
    list_studies(runs_dir).into_iter().find(|s| s.runs.iter().any(|r| r.id == run_id))
}

/// The study a new run made from `source_id` joins, with the source already in it, and the new run's label.
///
/// The study is `study_id`, else the source's own study, else a new one. A "Check it" study holds only repeats of
/// one run, so a new version of one of its runs starts a new study. Nothing is written: call [`join_study`] once
/// the new run exists.
pub fn plan_variant(
    runs_dir: &Path,
    source_id: &str,
    source: &SimConfig,
    study_id: Option<&str>,
    variant: Option<&str>,
    now: &str,
    source_label: Option<&str>,
) -> Result<(Study, String), StudyError> {
    let own = source.study_id.as_deref().and_then(|id| load_study(runs_dir, id));
    let repeat_source = own.as_ref().is_some_and(|s| s.kind == StudyKind::Repeat);
    let study_id = study_id
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| if repeat_source { None } else { source.study_id.clone() })
        .filter(|s| !s.is_empty());
    let mut study = study_id.as_deref().and_then(|id| load_study(runs_dir, id));
    if let (Some(id), None) = (&study_id, &study) {
        study_dir(runs_dir, id)?; // validates the id
        if Some(id) != source.study_id.as_ref() {
            return Err(StudyError::Invalid(format!("there is no study {id}")));
        }
    }
    if study.as_ref().is_some_and(|s| s.kind == StudyKind::Repeat) {
        return Err(StudyError::Invalid(
            "a Check it study holds only repeats of one run; a new version starts its own study".into(),
        ));
    }
    let study = study.get_or_insert_with(|| Study::new(&study_id.clone().unwrap_or_else(|| new_study_id(None)), now));
    if !study.runs.iter().any(|r| r.id == source_id) {
        // not "Run 2" of a check
        let own_label = if repeat_source { "" } else { py_strip(source.variant.as_deref().unwrap_or("")) };
        let label = [own_label, source_label.unwrap_or("")]
            .into_iter()
            .find(|l| !l.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| default_variant(study.runs.len() as i64));
        study.runs.push(StudyRun { id: source_id.into(), variant: label });
    }
    let label = py_prefix(py_strip(variant.unwrap_or("")), 60).to_string();
    let label = if label.is_empty() { default_variant(study.runs.len() as i64) } else { label };
    Ok((study.clone(), label))
}

/// Run `i + 1` of a "Check it" series of `source_id`: the same people (through `cast_from`), text and settings with
/// the next RNG seed. A repeat of a branch replays the branch's people and events from round 0, so it is not a
/// branch itself: it has no world to continue from.
pub fn repeat_config(
    source: &SimConfig,
    source_id: &str,
    i: i64,
    seed_files: &[PathBuf],
    study_id: &str,
    agent_model: Option<&str>,
) -> SimConfig {
    let mut config = source.clone();
    config.seed_files = seed_files.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    config.seed = source.seed + i;
    config.cast_from = Some(source_id.into());
    config.study_id = Some(study_id.into());
    config.variant = Some(format!("Run {}", i + 1));
    config.branched_from = None;
    config.branch_round = None;
    if let Some(model) = agent_model.filter(|m| !m.is_empty()) {
        config.agent_model = model.into();
    }
    config
}

/// Add `run_id` to `study` (or relabel it) and save the study.
pub fn join_study(runs_dir: &Path, study: &Study, run_id: &str, variant: &str) -> Result<Study, StudyError> {
    let mut runs: Vec<StudyRun> = study
        .runs
        .iter()
        .map(|r| if r.id == run_id { StudyRun { id: r.id.clone(), variant: variant.into() } } else { r.clone() })
        .collect();
    if !runs.iter().any(|r| r.id == run_id) {
        runs.push(StudyRun { id: run_id.into(), variant: variant.into() });
    }
    let joined = Study { runs, ..study.clone() };
    save_study(runs_dir, &joined)?;
    Ok(joined)
}
