//! Branch a run from any round: copy the run, keep rounds 0..r, change what happens next, and continue (port of
//! `tellfold/branch.py`).
//!
//! The copy shares rounds 0 to r with its source exactly: the same people, posts, reactions, follows and the
//! agents' memories as they were at the end of round r (kept per round in `memory_history`). New events replace the
//! source's events after round r, and the engine continues from round r + 1 (`SimConfig.branched_from` /
//! `branch_round`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use tellfold_models::{GodEvent, SimConfig};
use tellfold_store::files::{write_json, RunError, SEEDS_DIR};
use tellfold_store::world::{World, WorldError};

use crate::Error;

fn name_of(dir: &Path) -> String {
    dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// (last round, rounds a branch can start from): every round whose memories were kept, plus round 0 and the last
/// round, which older runs without a history can always use. A branch needs at least one round to run.
pub fn branch_rounds(source_dir: &Path, immutable: bool) -> Result<(i64, Vec<i64>), Error> {
    let db = source_dir.join("world.db");
    let world = if immutable { World::open_read_only(&db, 0)? } else { World::open(&db, 0)? };
    let read = (|| Ok::<_, WorldError>((world.last_round()?, world.memory_rounds()?)))();
    world.close()?;
    let (last, kept) = read?;
    let rounds: BTreeSet<i64> = [0, last].into_iter().chain(kept).collect();
    Ok((last, rounds.into_iter().collect()))
}

/// Where the branch comes from and what it joins.
#[derive(Debug, Clone, Default)]
pub struct BranchOptions<'a> {
    /// The source's run id when it is not its folder name (an example's `example-<folder>`).
    pub source_id: Option<&'a str>,
    pub immutable: bool,
    pub study_id: Option<&'a str>,
    pub variant: Option<&'a str>,
}

/// Fills `run_dir` (already created) with a copy of `source_dir` cut after `round`, and returns its config.
///
/// `events` are the branch's events after `round` (they replace the source's); events up to `round` stay as they
/// happened. Fails with [`Error::Run`] when the run cannot be branched there.
pub fn prepare_branch(
    source_dir: &Path,
    run_dir: &Path,
    source: &SimConfig,
    round: i64,
    events: &[GodEvent],
    options: BranchOptions<'_>,
) -> Result<SimConfig, Error> {
    let name = name_of(source_dir);
    let db = source_dir.join("world.db");
    if !db.is_file() {
        return Err(RunError(format!("{name} has no simulated world to branch")).into());
    }
    let (_, allowed) = branch_rounds(source_dir, options.immutable)?;
    if !allowed.contains(&round) {
        let list = allowed.iter().map(i64::to_string).collect::<Vec<_>>().join(", ");
        return Err(RunError(format!(
            "{name} can be branched from round {list} (it did not keep its agents' memories for round {round})"
        ))
        .into());
    }
    if round >= source.rounds {
        return Err(RunError(format!(
            "round {round} is the last round; a branch needs at least one round left to run"
        ))
        .into());
    }
    for e in events {
        if !(round < e.round && e.round <= source.rounds) {
            return Err(RunError(format!("event round {} is outside {}..{}", e.round, round + 1, source.rounds)).into());
        }
    }

    for file in ["graph.json", "personas.json"] {
        if source_dir.join(file).is_file() {
            std::fs::copy(source_dir.join(file), run_dir.join(file))?;
        }
    }
    let seeds = run_dir.join(SEEDS_DIR);
    if source_dir.join(SEEDS_DIR).is_dir() {
        copy_tree(&source_dir.join(SEEDS_DIR), &seeds)?;
    }
    copy_db(&db, &run_dir.join("world.db"), options.immutable)?;
    let world = World::open(&run_dir.join("world.db"), source.seed)?;
    let truncated = world.truncate_after(round);
    world.close()?;
    match truncated {
        Err(WorldError::Branch(message)) => return Err(RunError(message).into()),
        other => other?,
    }

    let source_id = options.source_id.map_or(name, str::to_string);
    let mut config = source.clone();
    if seeds.is_dir() {
        let mut files: Vec<PathBuf> =
            std::fs::read_dir(&seeds)?.map(|e| e.map(|e| e.path())).collect::<Result<_, _>>()?;
        files.sort();
        config.seed_files = files.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    }
    let mut later = events.to_vec();
    later.sort_by_key(|e| e.round);
    config.events = source.events.iter().filter(|e| e.round <= round).cloned().chain(later).collect();
    config.branched_from = Some(source_id.clone());
    config.branch_round = Some(round);
    config.cast_from = Some(source_id);
    config.study_id = options.study_id.map(str::to_string);
    config.variant = options.variant.map(str::to_string);
    // a branch continues a world that already has its people
    config.review_cast = false;
    config.audience = None;
    write_json(&run_dir.join("config.json"), &config)?;
    Ok(config)
}

/// `shutil.copytree(..., dirs_exist_ok=True)`.
pub(crate) fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        // `copytree(symlinks=False)` follows a link to a directory and copies what it points at.
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            copy_file(&entry.path(), &target)?;
        }
    }
    Ok(())
}

/// `shutil.copy2`: the bytes, the access and modification times, then the permissions. The times are set while
/// the copy is still writable, so a read-only source copies like any other.
pub(crate) fn copy_file(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut source = std::fs::File::open(from)?;
    let meta = source.metadata()?;
    let mut target = std::fs::File::create(to)?;
    std::io::copy(&mut source, &mut target)?;
    target.set_times(std::fs::FileTimes::new().set_accessed(meta.accessed()?).set_modified(meta.modified()?))?;
    drop(target);
    std::fs::set_permissions(to, meta.permissions())
}

/// Opens `path` read-only through a `file:` URI, like `sqlite3.connect(f"{uri}?mode=ro", uri=True)`.
pub(crate) fn open_source(path: &Path, immutable: bool) -> Result<Connection, Error> {
    let absolute = std::fs::canonicalize(path)?;
    let mode = if immutable { "ro&immutable=1" } else { "ro" };
    let uri = format!("file:{}?mode={mode}", uri_path(&absolute));
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    Ok(Connection::open_with_flags(uri, flags)?)
}

/// The path part of a `file:` URI: `%`, `?` and `#` and anything outside printable ASCII percent-encoded.
fn uri_path(path: &Path) -> String {
    let mut out = String::new();
    for b in path.to_string_lossy().bytes() {
        match b {
            b'%' | b'?' | b'#' | b' ' => out.push_str(&format!("%{b:02X}")),
            0x21..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A consistent copy through SQLite's backup API (it includes what is still in the source's WAL file).
fn copy_db(source: &Path, target: &Path, immutable: bool) -> Result<(), Error> {
    open_source(source, immutable)?.backup(rusqlite::DatabaseName::Main, target, None)?;
    Ok(())
}
