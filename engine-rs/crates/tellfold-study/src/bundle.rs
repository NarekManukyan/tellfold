//! `.tellfold` run bundles: one finished run as a single file another person can open.
//!
//! A bundle is a zip of a cleaned copy of the run folder, made the way [`pack_example`] makes a built-in example (no
//! WAL side files, no process id, relative seed paths, a calls log without prompt text), plus `bundle.json` naming the
//! format and the run. Nothing else may be in it: only the files of [`TOP_LEVEL`] and seed documents in `seeds/`.
//!
//! Importing never trusts the archive. Every name is checked before anything is written (no absolute paths, no `..`,
//! no backslashes, no links, no unexpected files, no duplicates, size limits that hold for what is actually read, not
//! only for what the archive claims), the files are unpacked into a hidden staging folder in the runs folder, the
//! JSON files must parse as their models and the world must be a plain SQLite database without triggers or views.
//! The config is written again from its model (without the sender's study), and `report.html` and `report.md` are
//! rendered again from `report.json`, so no page from the archive is ever shown. The run is then moved into place
//! with an `imported.json` next to it: the server treats such a run as read-only, like an example.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tellfold_llm::ingest::SUPPORTED_SUFFIXES;
use tellfold_models::{KnowledgeGraph, Persona, Report, RewriteSuggestion, RunState, RunStatus, SimConfig, VERSION};
use tellfold_report::html::render_html;
use tellfold_report::report::render_markdown;
use tellfold_store::files::{
    is_run_id, make_run_id, now_iso, write_json, write_status, CALLS_FILE, EXAMPLE_PREFIX, MAX_RUN_ID_LEN,
    REWRITE_FILE, SEEDS_DIR,
};
use tellfold_store::world::World;
use zip::read::ZipFile;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::branch::copy_file;
use crate::examples::{calls_hold_text, pack_example, KEEP};
use crate::Error;

/// The file extension of a bundle.
pub const EXTENSION: &str = "tellfold";
/// The manifest at the top of every bundle.
pub const MANIFEST_FILE: &str = "bundle.json";
/// Written next to an imported run; its presence makes the run read-only.
pub const IMPORTED_FILE: &str = "imported.json";
/// The newest bundle format this engine reads and the one it writes.
pub const FORMAT: i64 = 1;
/// The largest bundle made or accepted, as sent.
pub const MAX_BUNDLE_BYTES: u64 = 512 * 1024 * 1024;
/// The most a bundle may unpack to, all files together.
pub const MAX_UNPACKED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// The most entries (files and the seeds folder) a bundle may hold.
pub const MAX_ENTRIES: usize = 2000;
/// Files the run folder may hold at the top of a bundle.
pub const TOP_LEVEL: [&str; 11] = [
    MANIFEST_FILE,
    "config.json",
    "graph.json",
    "personas.json",
    "world.db",
    "calls.jsonl",
    "status.json",
    "report.json",
    "report.md",
    "report.html",
    REWRITE_FILE,
];
/// What a bundle must contain.
pub const REQUIRED: [&str; 6] =
    [MANIFEST_FILE, "config.json", "status.json", "report.json", "personas.json", "world.db"];

/// `bundle.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: i64,
    pub run_id: String,
    #[serde(default)]
    pub question: String,
    #[serde(default)]
    pub exported_at: String,
    #[serde(default)]
    pub engine_version: String,
}

/// `imported.json`: where an imported run came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Imported {
    pub imported_at: String,
    /// The run's id on the machine that exported it.
    pub source_run_id: String,
    #[serde(default)]
    pub exported_at: String,
    #[serde(default)]
    pub engine_version: String,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Bundle(message.into())
}

/// Whether `run_dir` holds a run imported from a bundle.
pub fn is_imported(run_dir: &Path) -> bool {
    run_dir.join(IMPORTED_FILE).is_file()
}

pub fn read_imported(run_dir: &Path) -> Option<Imported> {
    read_model(&run_dir.join(IMPORTED_FILE))
}

fn is_seed_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    SUPPORTED_SUFFIXES.iter().any(|s| lower.ends_with(s) && lower.len() > s.len())
}

// ---------- export ----------

/// Writes finished run `run_dir` (named `run_id`) as a bundle to `out`, through a temporary file next to it.
pub fn export_bundle(run_dir: &Path, run_id: &str, out: &Path) -> Result<(), Error> {
    let folder = out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut tmp = tempfile::Builder::new().prefix(".bundle-").suffix(".tmp").tempfile_in(folder)?;
    write_bundle(run_dir, run_id, tmp.as_file_mut())?;
    tmp.persist(out).map_err(|e| e.error)?;
    Ok(())
}

/// The bundle of finished run `run_dir` in an unnamed temporary file, read from its start. The file has no name on
/// disk, so nothing is left behind however the reader ends.
pub fn bundle_file(run_dir: &Path, run_id: &str) -> Result<std::fs::File, Error> {
    let mut file = tempfile::tempfile()?;
    write_bundle(run_dir, run_id, &mut file)?;
    file.seek(SeekFrom::Start(0))?;
    Ok(file)
}

/// The bundle of finished run `run_dir` as bytes.
pub fn bundle_bytes(run_dir: &Path, run_id: &str) -> Result<Vec<u8>, Error> {
    let mut out = std::io::Cursor::new(Vec::new());
    write_bundle(run_dir, run_id, &mut out)?;
    Ok(out.into_inner())
}

const MIB: u64 = 1024 * 1024;

/// The sizes of all files under `dir`, added up.
fn folder_bytes(dir: &Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        total = total.saturating_add(if meta.is_dir() { folder_bytes(&entry.path())? } else { meta.len() });
    }
    Ok(total)
}

/// Writes finished run `run_dir` as a bundle to `writer`. Refuses a run that is not finished, and one whose bundle
/// another Tellfold would refuse for its size.
pub fn write_bundle<W: Write + Seek>(run_dir: &Path, run_id: &str, writer: W) -> Result<(), Error> {
    let finished = read_model::<RunStatus>(&run_dir.join("status.json")).is_some_and(|s| s.state == RunState::Done);
    if !finished || !run_dir.join("report.json").is_file() {
        return Err(invalid("only a finished run with a report can be shared"));
    }
    // checked here, before the copy, so the message names no folder of this machine
    let calls = calls_hold_text(&run_dir.join(CALLS_FILE)).map_err(|_| invalid("the run's calls log is damaged"))?;
    if calls {
        return Err(invalid("the run's calls log holds prompt or answer text, so it cannot be shared"));
    }
    let staging = tempfile::tempdir()?;
    let clean = staging.path().join("run");
    pack_example(run_dir, &clean).map_err(|e| match e {
        Error::Example(message) => invalid(message.replace("cannot ship", "cannot be shared")),
        other => other,
    })?;
    if run_dir.join(REWRITE_FILE).is_file() {
        copy_file(&run_dir.join(REWRITE_FILE), &clean.join(REWRITE_FILE))?;
    }
    let unpacked = folder_bytes(&clean)?;
    if unpacked > MAX_UNPACKED_BYTES {
        return Err(invalid(format!(
            "this run is too large to share: it holds {} MB, and a bundle may unpack to at most {} MB",
            unpacked.div_ceil(MIB),
            MAX_UNPACKED_BYTES / MIB
        )));
    }
    let config: SimConfig = parse_file(&clean, "config.json")?;
    let manifest = Manifest {
        format: FORMAT,
        run_id: run_id.to_string(),
        question: config.question,
        exported_at: now_iso(),
        engine_version: VERSION.to_string(),
    };

    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated).unix_permissions(0o644);
    let mut zip = ZipWriter::new(writer);
    zip.start_file(MANIFEST_FILE, options)?;
    zip.write_all(tellfold_store::files::json_text(&manifest).as_bytes())?;
    for name in KEEP.iter().copied().chain([REWRITE_FILE]) {
        let path = clean.join(name);
        if path.is_file() {
            zip.start_file(name, options)?;
            std::io::copy(&mut std::fs::File::open(&path)?, &mut zip)?;
        }
    }
    let mut seeds: Vec<String> = std::fs::read_dir(clean.join(SEEDS_DIR))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| e.path().is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| !n.starts_with('.') && is_seed_name(n))
                .collect()
        })
        .unwrap_or_default();
    seeds.sort();
    zip.add_directory(format!("{SEEDS_DIR}/"), options.unix_permissions(0o755))?;
    for name in seeds {
        zip.start_file(format!("{SEEDS_DIR}/{name}"), options)?;
        std::io::copy(&mut std::fs::File::open(clean.join(SEEDS_DIR).join(&name))?, &mut zip)?;
    }
    let mut writer = zip.finish()?;
    let size = writer.stream_position()?;
    if size > MAX_BUNDLE_BYTES {
        return Err(invalid(format!(
            "this run is too large to share: its bundle would be {} MB, and Tellfold opens bundles of up to {} MB",
            size.div_ceil(MIB),
            MAX_BUNDLE_BYTES / MIB
        )));
    }
    Ok(())
}

// ---------- import ----------

/// The start of the hidden folder an import unpacks into, in the runs folder.
pub const STAGING_PREFIX: &str = ".import-";
/// A staging folder this old was left by an import that never finished.
const STALE_STAGING: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Removes staging folders in `runs_dir` left by imports that never finished (the engine stopped halfway). A folder
/// younger than an hour is kept: another engine on the same runs folder may still be unpacking it.
pub fn sweep_staging(runs_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(runs_dir) else { return };
    for entry in entries.filter_map(Result::ok) {
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().is_ok_and(|age| age >= STALE_STAGING));
        if stale && entry.file_name().to_string_lossy().starts_with(STAGING_PREFIX) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Where an archive entry goes in the run folder, or why it is refused.
fn entry_path(name: &str) -> Result<Option<PathBuf>, Error> {
    let shown: String = name.chars().take(120).collect();
    if name.is_empty() || name.starts_with('/') || name.contains('\\') || name.contains('\0') {
        return Err(invalid(format!("the bundle holds a file with an unsafe name: {shown:?}")));
    }
    if name.split('/').any(|part| part == ".." || part == ".") || name.chars().any(char::is_control) {
        return Err(invalid(format!("the bundle holds a file with an unsafe name: {shown:?}")));
    }
    if name == format!("{SEEDS_DIR}/") {
        return Ok(None); // the folder itself
    }
    if TOP_LEVEL.contains(&name) {
        return Ok(Some(PathBuf::from(name)));
    }
    if let Some(seed) = name.strip_prefix(&format!("{SEEDS_DIR}/")) {
        if !seed.is_empty() && !seed.contains('/') && !seed.starts_with('.') && seed.len() <= 255 && is_seed_name(seed)
        {
            return Ok(Some(Path::new(SEEDS_DIR).join(seed)));
        }
    }
    Err(invalid(format!("the bundle holds an unexpected file: {shown:?}")))
}

fn read_model<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_reader(BufReader::new(std::fs::File::open(path).ok()?)).ok()
}

fn parse_file<T: serde::de::DeserializeOwned>(dir: &Path, name: &str) -> Result<T, Error> {
    let file = std::fs::File::open(dir.join(name)).map_err(|_| invalid(format!("{name} is not readable")))?;
    serde_json::from_reader(BufReader::new(file)).map_err(|e| invalid(format!("{name} is not valid: {e}")))
}

/// A run id from the bundle that is safe as a folder name and cannot pass for an example's.
fn safe_run_id(id: &str) -> bool {
    is_run_id(id) && !id.contains("..") && !id.starts_with(EXAMPLE_PREFIX)
}

/// Unpacks bundle `reader` as a new, read-only run in `runs_dir`. Returns the new run's id: the exported run's id, or
/// that id with `-2`, `-3`, ... when a run of that name is already there.
///
/// Each step is one kind of check: [`check_entries`] looks at the names and sizes the archive claims, [`unpack`]
/// holds the files to what is actually read, [`validate_run`] parses what was unpacked, and [`finish`] writes the
/// files the receiver's engine owns before [`place`] moves the run into the runs folder.
pub fn import_bundle<R: Read + Seek>(reader: R, runs_dir: &Path) -> Result<String, Error> {
    let mut archive = ZipArchive::new(reader).map_err(|e| invalid(format!("this is not a Tellfold bundle: {e}")))?;
    let targets = check_entries(&mut archive)?;
    std::fs::create_dir_all(runs_dir)?;
    // hidden, so the run list never shows a half-unpacked run; one an engine crash leaves is swept on the next start
    let staging = tempfile::Builder::new().prefix(STAGING_PREFIX).tempdir_in(runs_dir)?;
    unpack(&mut archive, &targets, staging.path())?;
    let run = validate_run(staging.path())?;
    let base = finish(staging.path(), run)?;
    let id = place(staging.path(), runs_dir, &base)?;
    drop(staging); // already moved: nothing left to remove
    Ok(id)
}

/// The name of one archive entry and where it goes (`None` for the seeds folder). Refuses encrypted entries, links,
/// unsafe or unexpected names and files posing as folders.
fn check_entry<R: Read>(file: &ZipFile<'_, R>) -> Result<(String, Option<PathBuf>), Error> {
    let name = String::from_utf8(file.name_raw().to_vec())
        .map_err(|_| invalid("the bundle holds a file whose name is not UTF-8"))?;
    if file.encrypted() {
        return Err(invalid("the bundle holds an encrypted file"));
    }
    if file.is_symlink() || file.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
        return Err(invalid(format!("the bundle holds a link: {name:?}")));
    }
    let target = entry_path(&name)?;
    if file.is_dir() != target.is_none() {
        return Err(invalid(format!("the bundle holds an unexpected folder: {name:?}")));
    }
    Ok((name, target))
}

/// Every entry's target, checked before anything is written: at most [`MAX_ENTRIES`] entries, no name twice (in any
/// case), the [`REQUIRED`] files present, and claimed sizes within [`MAX_UNPACKED_BYTES`].
fn check_entries<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<Vec<Option<PathBuf>>, Error> {
    if archive.len() > MAX_ENTRIES {
        return Err(invalid(format!("the bundle holds more than {MAX_ENTRIES} files")));
    }
    let mut targets = Vec::with_capacity(archive.len());
    let mut seen = HashSet::new();
    let mut declared: u64 = 0;
    for i in 0..archive.len() {
        let file = archive.by_index(i).map_err(|e| invalid(format!("the bundle is damaged: {e}")))?;
        let (name, target) = check_entry(&file)?;
        if !seen.insert(name.to_lowercase()) {
            return Err(invalid(format!("the bundle holds {name:?} twice")));
        }
        declared = declared.saturating_add(file.size());
        targets.push(target);
    }
    if declared > MAX_UNPACKED_BYTES {
        return Err(invalid("the bundle is too large to unpack"));
    }
    if let Some(missing) = REQUIRED.iter().find(|name| !seen.contains(**name)) {
        return Err(invalid(format!("the bundle has no {missing}")));
    }
    Ok(targets)
}

/// Writes each entry to its target in `dir`. The size limit holds for the bytes actually inflated, not for what the
/// archive claims, and an entry must be exactly the size it claims.
fn unpack<R: Read + Seek>(archive: &mut ZipArchive<R>, targets: &[Option<PathBuf>], dir: &Path) -> Result<(), Error> {
    std::fs::create_dir(dir.join(SEEDS_DIR))?;
    let mut budget = MAX_UNPACKED_BYTES;
    for (i, target) in targets.iter().enumerate() {
        let Some(target) = target else { continue };
        let file = archive.by_index(i).map_err(|e| invalid(format!("the bundle is damaged: {e}")))?;
        let claimed = file.size();
        let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(dir.join(target))?;
        // Read one byte past what may be left: a file that says it is small but is not is caught here.
        let written = std::io::copy(&mut file.take(budget.saturating_add(1)), &mut out)
            .map_err(|e| invalid(format!("the bundle is damaged: {e}")))?;
        if written > budget || written != claimed {
            return Err(invalid("the bundle is too large to unpack or its sizes are wrong"));
        }
        budget -= written;
    }
    Ok(())
}

/// The parsed files of an unpacked run that [`finish`] writes again.
struct Unpacked {
    manifest: Manifest,
    config: SimConfig,
    status: RunStatus,
    report: Report,
}

/// Every JSON file of the run in `dir` parses as its model, the run is finished, and its world is a plain database.
fn validate_run(dir: &Path) -> Result<Unpacked, Error> {
    let manifest: Manifest = parse_file(dir, MANIFEST_FILE)?;
    if manifest.format < 1 || manifest.format > FORMAT {
        return Err(invalid(format!(
            "this bundle is format {}; this version of Tellfold reads format {FORMAT}. Update Tellfold to open it",
            manifest.format
        )));
    }
    let config: SimConfig = parse_file(dir, "config.json")?;
    let status: RunStatus = parse_file(dir, "status.json")?;
    let report: Report = parse_file(dir, "report.json")?;
    let _: Vec<Persona> = parse_file(dir, "personas.json")?;
    if dir.join("graph.json").is_file() {
        let _: KnowledgeGraph = parse_file(dir, "graph.json")?;
    }
    if dir.join(REWRITE_FILE).is_file() {
        let _: RewriteSuggestion = parse_file(dir, REWRITE_FILE)?;
    }
    if dir.join("calls.jsonl").is_file() {
        check_calls(&dir.join("calls.jsonl"))?;
    }
    if status.state != RunState::Done {
        return Err(invalid("only a finished run can be imported"));
    }
    check_world(&dir.join("world.db"))?;
    Ok(Unpacked { manifest, config, status, report })
}

/// Each line of the calls log is a JSON value, read one line at a time.
fn check_calls(path: &Path) -> Result<(), Error> {
    for line in BufReader::new(std::fs::File::open(path)?).lines() {
        let line = line.map_err(|_| invalid("calls.jsonl is not text"))?;
        if !line.trim().is_empty() && serde_json::from_str::<serde::de::IgnoredAny>(&line).is_err() {
            return Err(invalid("calls.jsonl is not valid"));
        }
    }
    Ok(())
}

/// Writes again the files this engine owns: the config without the sender's study, versions or seed paths, the
/// status without a process id, the report pages rendered from `report.json`, and `imported.json`. Returns the id
/// the run should take.
fn finish(dir: &Path, run: Unpacked) -> Result<String, Error> {
    let Unpacked { manifest, mut config, status, report } = run;
    config.study_id = None;
    config.variant = None;
    let mut seeds: Vec<String> = std::fs::read_dir(dir.join(SEEDS_DIR))?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    seeds.sort();
    config.seed_files = seeds.iter().map(|n| format!("{SEEDS_DIR}/{n}")).collect();
    write_json(&dir.join("config.json"), &config)?;
    write_status(dir, &RunStatus { pid: None, ..status })?;
    let world = World::open_read_only(&dir.join("world.db"), config.seed)?;
    let html = render_html(&report, &world, &config);
    world.close()?;
    std::fs::write(dir.join("report.html"), html?)?;
    std::fs::write(dir.join("report.md"), render_markdown(&report))?;
    write_json(
        &dir.join(IMPORTED_FILE),
        &Imported {
            imported_at: now_iso(),
            source_run_id: manifest.run_id.clone(),
            exported_at: manifest.exported_at,
            engine_version: manifest.engine_version,
        },
    )?;
    Ok(if safe_run_id(&manifest.run_id) { manifest.run_id } else { make_run_id(&config.question, None) })
}

/// `base`, or `base` cut short with `-2`, `-3`, ... so it stays a valid run id, when that name is taken.
fn candidate_id(base: &str, n: u32) -> String {
    if n < 2 {
        return base.to_string();
    }
    let suffix = format!("-{n}");
    // ASCII only (a safe run id or a made one), so any length is a character boundary
    let keep = base.len().min(MAX_RUN_ID_LEN - suffix.len());
    format!("{}{suffix}", &base[..keep])
}

/// Moves unpacked run `dir` into `runs_dir` under the first free [`candidate_id`]. The name is taken with
/// `create_dir` first, so a run another request is creating at the same moment is never replaced; the rename then
/// replaces only that new, empty folder.
fn place(dir: &Path, runs_dir: &Path, base: &str) -> Result<String, Error> {
    for n in 1..=10_000 {
        let id = candidate_id(base, n);
        let target = runs_dir.join(&id);
        match std::fs::create_dir(&target) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
        if let Err(e) = std::fs::rename(dir, &target) {
            let _ = std::fs::remove_dir(&target);
            return Err(e.into());
        }
        return Ok(id);
    }
    Err(invalid("there are too many runs with this bundle's name"))
}

/// A world from a bundle must be a SQLite database with Tellfold's tables, no triggers or views, that passes a quick
/// integrity check. It is switched to a rollback journal so it can be opened read-only without side files.
fn check_world(db: &Path) -> Result<(), Error> {
    let mut header = [0u8; 16];
    std::fs::File::open(db)?.read_exact(&mut header).map_err(|_| invalid("world.db is not a database"))?;
    if &header != b"SQLite format 3\0" {
        return Err(invalid("world.db is not a database"));
    }
    let bad = |e: rusqlite::Error| invalid(format!("world.db is not a valid database: {e}"));
    let conn = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(bad)?;
    conn.execute_batch("PRAGMA trusted_schema=OFF").map_err(bad)?;
    let kinds: Vec<(String, String)> = conn
        .prepare("SELECT type, name FROM sqlite_master")
        .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
        .map_err(bad)?;
    if kinds.iter().any(|(kind, _)| kind == "trigger" || kind == "view") {
        return Err(invalid("world.db holds triggers or views"));
    }
    for table in ["personas", "posts", "actions", "round_stats"] {
        if !kinds.iter().any(|(kind, name)| kind == "table" && name == table) {
            return Err(invalid(format!("world.db has no {table} table")));
        }
    }
    let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0)).map_err(bad)?;
    if check != "ok" {
        return Err(invalid("world.db is damaged"));
    }
    conn.query_row("PRAGMA journal_mode=DELETE", [], |_| Ok(())).map_err(bad)?;
    conn.close().map_err(|(_, e)| bad(e))?;
    for side in ["world.db-wal", "world.db-shm", "world.db-journal"] {
        let _ = std::fs::remove_file(db.with_file_name(side));
    }
    Ok(())
}
