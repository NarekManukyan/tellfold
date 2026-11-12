//! `.tellfold` bundles: a finished run survives the round trip, and a bundle that tries to write outside its run, hides
//! a link or an unexpected file, or carries a hostile database is refused without leaving anything behind.

use std::future::Future;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;
use tellfold_engine::runner::{run_pipeline, RunOptions};
use tellfold_llm::canned::fake_responder;
use tellfold_llm::fake::FakeLlm;
use tellfold_models::SimConfig;
use tellfold_report::ReportWriter;
use tellfold_store::files::copy_seeds;
use tellfold_study::bundle::{bundle_bytes, export_bundle, import_bundle, is_imported, read_imported, IMPORTED_FILE};
use tellfold_study::Error;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

fn block_on<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}

fn seed_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../examples/mossbook_pricing.md")
}

/// A finished canned run of the mossbook seed, in a study, with absolute seed paths in its config.
fn finished_run(runs: &Path, name: &str) -> PathBuf {
    let run_dir = runs.join(name);
    std::fs::create_dir_all(&run_dir).unwrap();
    let seeds = copy_seeds(&run_dir, &[seed_path()]).unwrap();
    let mut config = SimConfig::new(
        "Will buyers accept the subscription?",
        seeds.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
    );
    config.num_agents = 6;
    config.rounds = 2;
    config.batch_size = 3;
    config.study_id = Some("20261110-090000-abcd".into());
    config.variant = Some("Draft B".into());
    let llm = FakeLlm::new(fake_responder);
    block_on(run_pipeline(&llm, &run_dir, &config, &seeds, &ReportWriter, RunOptions::default())).unwrap().unwrap();
    run_dir
}

fn json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn posts(db: &Path) -> Vec<(i64, String)> {
    let conn = rusqlite::Connection::open(db).unwrap();
    let mut stmt = conn.prepare("SELECT id, content FROM posts ORDER BY id").unwrap();
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    rows.map(Result::unwrap).collect()
}

/// The entries of a bundle, in order.
fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    (0..archive.len())
        .map(|i| {
            let mut file = archive.by_index(i).unwrap();
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            (file.name().unwrap().to_string(), data)
        })
        .collect()
}

/// A zip of `files`, names written as given.
fn zip_of(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in files {
        if name.ends_with('/') {
            zip.add_directory(name.as_str(), SimpleFileOptions::default()).unwrap();
        } else {
            zip.start_file(name.as_str(), SimpleFileOptions::default()).unwrap();
            zip.write_all(data).unwrap();
        }
    }
    zip.finish().unwrap().into_inner()
}

fn refused(bytes: Vec<u8>, runs: &Path) -> String {
    match import_bundle(Cursor::new(bytes), runs) {
        Err(Error::Bundle(message)) => message,
        Err(other) => panic!("refused for the wrong reason: {other}"),
        Ok(id) => panic!("a bad bundle was imported as {id}"),
    }
}

/// What is in `dir` (names only), sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .map(|it| it.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    out.sort();
    out
}

#[test]
fn a_finished_run_survives_the_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let source = finished_run(&tmp.path().join("mine"), "20261110-090604-will-buyers");
    let file = tmp.path().join("shared.tellfold");
    export_bundle(&source, "20261110-090604-will-buyers", &file).unwrap();

    let bytes = std::fs::read(&file).unwrap();
    let listed: Vec<String> = entries(&bytes).into_iter().map(|(n, _)| n).collect();
    assert_eq!(listed[0], "bundle.json");
    for name in ["config.json", "status.json", "report.json", "report.html", "personas.json", "world.db", "seeds/"] {
        assert!(listed.iter().any(|n| n == name), "{name} missing from {listed:?}");
    }
    assert!(listed.contains(&"seeds/mossbook_pricing.md".to_string()));
    assert!(!listed.iter().any(|n| n.contains("-wal") || n.contains("-shm")), "{listed:?}");
    let source_dir = source.to_string_lossy().into_owned();
    for (name, data) in entries(&bytes) {
        assert!(!String::from_utf8_lossy(&data).contains(&source_dir), "{name} names the sender's folder");
    }

    let theirs = tmp.path().join("theirs");
    let id = import_bundle(std::fs::File::open(&file).unwrap(), &theirs).unwrap();
    assert_eq!(id, "20261110-090604-will-buyers");
    let run = theirs.join(&id);
    assert!(is_imported(&run));
    assert_eq!(read_imported(&run).unwrap().source_run_id, id);
    assert_eq!(names(&theirs), vec![id.clone()], "no staging folder is left behind");

    let config = json(&run.join("config.json"));
    assert_eq!(config["question"], "Will buyers accept the subscription?");
    assert_eq!(config["seed_files"], serde_json::json!(["seeds/mossbook_pricing.md"]));
    assert!(config["study_id"].is_null() && config["variant"].is_null(), "the sender's study stays behind");
    assert_eq!(json(&run.join("report.json")), json(&source.join("report.json")));
    assert_eq!(json(&run.join("personas.json")), json(&source.join("personas.json")));
    assert!(json(&run.join("status.json"))["pid"].is_null());
    assert_eq!(posts(&run.join("world.db")), posts(&source.join("world.db")));
    assert_eq!(std::fs::read(run.join("seeds/mossbook_pricing.md")).unwrap(), std::fs::read(seed_path()).unwrap());
    assert!(std::fs::read_to_string(run.join("report.html")).unwrap().contains("<html"));
    assert!(!run.join("world.db-wal").exists());

    // opening the same bundle again makes a second copy, never overwrites the first
    let again = import_bundle(Cursor::new(bytes.clone()), &theirs).unwrap();
    assert_eq!(again, "20261110-090604-will-buyers-2");
    // and an imported run can be shared on
    let onward = bundle_bytes(&run, &id).unwrap();
    assert!(import_bundle(Cursor::new(onward), &tmp.path().join("third")).is_ok());
}

#[test]
fn a_grounded_run_carries_its_quotes_but_no_file_name() {
    use tellfold_engine::material::{read_material, write_material, MaterialFile};
    let tmp = tempfile::tempdir().unwrap();
    let run_dir = tmp.path().join("mine/20261110-090604-grounded");
    std::fs::create_dir_all(&run_dir).unwrap();
    let seeds = copy_seeds(&run_dir, &[seed_path()]).unwrap();
    let reviews: String =
        (0..12).map(|i| format!("Review {i}: the yearly plan doubled and I may leave for another app.\n\n")).collect();
    let file = MaterialFile {
        name: "Quillfeather family.txt".into(),
        kind: Some("reviews".into()),
        column: None,
        bytes: reviews.into(),
    };
    let material = read_material(&[file], &[], None).unwrap();
    write_material(&run_dir, &material).unwrap();
    let mut config = SimConfig::new(
        "Will buyers accept the subscription?",
        seeds.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
    );
    (config.num_agents, config.rounds, config.batch_size) = (6, 2, 3);
    config.material = Some(material.summary);
    let llm = FakeLlm::new(fake_responder);
    block_on(run_pipeline(&llm, &run_dir, &config, &seeds, &ReportWriter, RunOptions::default())).unwrap().unwrap();

    let bytes = bundle_bytes(&run_dir, "20261110-090604-grounded").unwrap();
    let files = entries(&bytes);
    // the redacted items stay with the sender; the file they came from is named by label only
    assert!(!files.iter().any(|(n, _)| n == "material.json"));
    for (name, data) in &files {
        assert!(!String::from_utf8_lossy(data).contains("Quillfeather"), "{name} names the material's file");
    }
    // the people keep the quotes they were written from (docs/API.md says so, and the app's export panel)
    let personas = &files.iter().find(|(n, _)| n == "personas.json").unwrap().1;
    assert!(String::from_utf8_lossy(personas).contains("\"quotes\""));
    let config = &files.iter().find(|(n, _)| n == "config.json").unwrap().1;
    let config: Value = serde_json::from_slice(config).unwrap();
    assert_eq!(config["material"]["files"], serde_json::json!(["reviews 1"]));
}

#[test]
fn only_a_finished_run_can_be_shared() {
    let tmp = tempfile::tempdir().unwrap();
    let source = finished_run(&tmp.path().join("mine"), "run");
    std::fs::remove_file(source.join("report.json")).unwrap();
    let error = bundle_bytes(&source, "run").unwrap_err().to_string();
    assert!(error.contains("only a finished run with a report"), "{error}");
    // the message is shown in the app: it names no folder of this machine
    assert!(!error.contains(&tmp.path().to_string_lossy().into_owned()), "{error}");
}

#[test]
fn a_calls_log_with_prompt_text_is_refused_without_naming_a_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let source = finished_run(&tmp.path().join("mine"), "run");
    let mut calls = std::fs::OpenOptions::new().create(true).append(true).open(source.join("calls.jsonl")).unwrap();
    writeln!(calls, "{{\"purpose\": \"agent.post\", \"prompt\": \"the whole prompt\"}}").unwrap();
    let error = bundle_bytes(&source, "run").unwrap_err().to_string();
    assert_eq!(error, "the run's calls log holds prompt or answer text, so it cannot be shared");
}

/// Sets the uncompressed size `name` claims, in its local header and in the central directory, to `size`.
fn claim_size(bytes: &mut [u8], name: &str, size: u32) {
    let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]) as usize;
    let mut patched = 0;
    for i in 0..bytes.len().saturating_sub(46) {
        let (field, name_len, name_at) = match &bytes[i..i + 4] {
            [0x50, 0x4b, 0x03, 0x04] => (i + 22, u16_at(bytes, i + 26), i + 30),
            [0x50, 0x4b, 0x01, 0x02] => (i + 24, u16_at(bytes, i + 28), i + 46),
            _ => continue,
        };
        if bytes.get(name_at..name_at + name_len) == Some(name.as_bytes()) {
            bytes[field..field + 4].copy_from_slice(&size.to_le_bytes());
            patched += 1;
        }
    }
    assert_eq!(patched, 2, "{name} is in the local headers and the central directory");
}

#[test]
fn sizes_and_counts_are_held_to_what_is_actually_there() {
    let tmp = tempfile::tempdir().unwrap();
    let source = finished_run(&tmp.path().join("mine"), "20261110-090604-will-buyers");
    let good = entries(&bundle_bytes(&source, "20261110-090604-will-buyers").unwrap());
    let runs = tmp.path().join("runs");

    // a file that says it is one byte long but inflates to more (the zip reader or the count of what was written
    // catches it, whichever is first)
    let mut understated = zip_of(&good);
    claim_size(&mut understated, "report.md", 1);
    let message = refused(understated, &runs);
    assert!(message.contains("sizes are wrong") || message.contains("declared uncompressed size"), "{message}");

    // a file that claims one byte more than it holds: only the count of what was written catches it
    let real = good.iter().find(|(n, _)| n == "report.md").unwrap().1.len() as u32;
    let mut overclaimed = zip_of(&good);
    claim_size(&mut overclaimed, "report.md", real + 1);
    assert_eq!(refused(overclaimed, &runs), "the bundle is too large to unpack or its sizes are wrong");

    // files that claim more than may be unpacked are refused before anything is written
    let mut overstated = zip_of(&good);
    claim_size(&mut overstated, "report.md", u32::MAX);
    assert!(refused(overstated, &runs).contains("too large to unpack"));

    // more entries than a bundle may hold
    let mut crowded = good.clone();
    crowded.extend((0..tellfold_study::bundle::MAX_ENTRIES).map(|i| (format!("seeds/note-{i}.md"), b"x".to_vec())));
    assert!(refused(zip_of(&crowded), &runs).contains("more than"));

    assert!(names(&runs).is_empty(), "left behind: {:?}", names(&runs));
}

#[test]
fn a_long_id_stays_a_valid_run_id_when_it_needs_a_number() {
    let tmp = tempfile::tempdir().unwrap();
    let source = finished_run(&tmp.path().join("mine"), "20261110-090604-will-buyers");
    let long = format!("20261110-090604-{}", "a".repeat(184));
    assert_eq!(long.len(), tellfold_store::files::MAX_RUN_ID_LEN);
    let bytes = bundle_bytes(&source, &long).unwrap();
    let runs = tmp.path().join("runs");
    assert_eq!(import_bundle(Cursor::new(bytes.clone()), &runs).unwrap(), long);
    for n in 2..=11 {
        let id = import_bundle(Cursor::new(bytes.clone()), &runs).unwrap();
        assert!(tellfold_store::files::is_run_id(&id) && id.ends_with(&format!("-{n}")), "{id}");
    }
}

#[test]
fn staging_left_by_a_stopped_import_is_swept() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path();
    let stale = runs.join(".import-stale");
    let fresh = runs.join(".import-fresh");
    let older = runs.join(".import-older");
    for dir in [&stale, &fresh, &older] {
        std::fs::create_dir_all(dir.join("seeds")).unwrap();
    }
    let hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 60 * 60);
    for dir in [&stale, &older] {
        std::fs::File::open(dir).unwrap().set_modified(hours_ago).unwrap();
    }
    std::fs::create_dir_all(runs.join("20261110-090604-kept")).unwrap();
    tellfold_study::bundle::sweep_staging(runs);
    assert_eq!(names(runs), vec![".import-fresh", "20261110-090604-kept"]);
}

#[test]
fn hostile_bundles_are_refused_and_leave_nothing_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let source = finished_run(&tmp.path().join("mine"), "20261110-090604-will-buyers");
    let good = entries(&bundle_bytes(&source, "20261110-090604-will-buyers").unwrap());
    let runs = tmp.path().join("box/runs");
    std::fs::create_dir_all(&runs).unwrap();
    let with = |extra: (&str, &[u8])| {
        let mut files = good.clone();
        files.push((extra.0.to_string(), extra.1.to_vec()));
        zip_of(&files)
    };
    let replacing = |name: &str, data: &[u8]| {
        let files: Vec<(String, Vec<u8>)> =
            good.iter().map(|(n, d)| (n.clone(), if n == name { data.to_vec() } else { d.clone() })).collect();
        zip_of(&files)
    };

    for (name, why) in [
        ("../escaped.md", "unsafe name"),
        ("seeds/../../escaped.md", "unsafe name"),
        ("/tmp/escaped.md", "unsafe name"),
        ("seeds\\..\\escaped.md", "unsafe name"),
        ("./config.json", "unsafe name"),
        ("notes.txt", "unexpected file"),
        ("seeds/install.sh", "unexpected file"),
        ("seeds/nested/a.md", "unexpected file"),
        ("seeds/.hidden.md", "unexpected file"),
        ("world.db-wal", "unexpected file"),
        ("seeds/MOSSBOOK_PRICING.md", "twice"),
    ] {
        let message = refused(with((name, b"x")), &runs);
        assert!(message.contains(why), "{name}: {message}");
    }
    assert!(refused(with(("extra/", b"")), &runs).contains("unexpected"));

    // a symbolic link, even under an allowed name
    let mut files = good.clone();
    files.retain(|(n, _)| n != "seeds/mossbook_pricing.md");
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in &files {
        if name.ends_with('/') {
            zip.add_directory(name.as_str(), SimpleFileOptions::default()).unwrap();
        } else {
            zip.start_file(name.as_str(), SimpleFileOptions::default()).unwrap();
            zip.write_all(data).unwrap();
        }
    }
    zip.add_symlink("seeds/mossbook_pricing.md", "/etc/passwd", SimpleFileOptions::default()).unwrap();
    assert!(refused(zip.finish().unwrap().into_inner(), &runs).contains("link"));

    // missing parts and broken contents
    let without_world: Vec<_> = good.iter().filter(|(n, _)| n != "world.db").cloned().collect();
    assert!(refused(zip_of(&without_world), &runs).contains("no world.db"));
    assert!(refused(replacing("world.db", b"not a database at all"), &runs).contains("world.db"));
    assert!(refused(replacing("config.json", b"{\"question\": 3}"), &runs).contains("config.json"));
    assert!(refused(replacing("bundle.json", b"{\"format\": 99, \"run_id\": \"x\"}"), &runs).contains("format 99"));
    let unfinished = String::from_utf8(good.iter().find(|(n, _)| n == "status.json").unwrap().1.clone())
        .unwrap()
        .replace("\"done\"", "\"running\"");
    assert!(refused(replacing("status.json", unfinished.as_bytes()), &runs).contains("finished"));
    assert!(refused(b"PK\x03\x04 not really a zip".to_vec(), &runs).contains("not a Tellfold bundle"));

    // a database that would run code of its own when written to
    let db = tmp.path().join("trap.db");
    std::fs::write(&db, &good.iter().find(|(n, _)| n == "world.db").unwrap().1).unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("CREATE TRIGGER trap AFTER INSERT ON posts BEGIN DELETE FROM personas; END;").unwrap();
    conn.close().unwrap();
    assert!(refused(replacing("world.db", &std::fs::read(&db).unwrap()), &runs).contains("triggers"));

    assert!(names(&runs).is_empty(), "left behind: {:?}", names(&runs));
    assert_eq!(names(&tmp.path().join("box")), vec!["runs"], "nothing escaped the runs folder");

    // the untouched bundle still opens; an id that could pass for an example's is replaced
    let renamed: Vec<(String, Vec<u8>)> = good
        .iter()
        .map(|(n, d)| {
            let d = if n == "bundle.json" {
                String::from_utf8_lossy(d).replace("20261110-090604-will-buyers", "example-corner-cafe").into_bytes()
            } else {
                d.clone()
            };
            (n.clone(), d)
        })
        .collect();
    let id = import_bundle(Cursor::new(zip_of(&renamed)), &runs).unwrap();
    assert!(!id.starts_with("example-") && id.ends_with("-will-buyers-accept-the-subscription"), "{id}");
    assert!(runs.join(&id).join(IMPORTED_FILE).is_file());
}
