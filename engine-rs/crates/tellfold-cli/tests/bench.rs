//! The accuracy benchmark's whole dry run on this CLI's offline model: run, grade, results JSON and Markdown, then a
//! resumed pass that keeps every finished run.

use std::path::Path;

use serde_json::Value;

fn bench(tmp: &Path, extra: &[&str]) -> i32 {
    let cases = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tellfold-bench/tests/fixtures");
    let mut args: Vec<String> = vec![
        "--dry-run".into(),
        "--zero-shot".into(),
        "--repeats".into(),
        "2".into(),
        "--tellfold".into(),
        env!("CARGO_BIN_EXE_tellfold").into(),
        "--cases-dir".into(),
        cases.display().to_string(),
        "--results-dir".into(),
        tmp.join("results").display().to_string(),
        "--runs-dir".into(),
        tmp.join("runs").display().to_string(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    tellfold_bench::main(&args)
}

fn results(tmp: &Path) -> Vec<std::path::PathBuf> {
    let mut found: Vec<_> = std::fs::read_dir(tmp.join("results"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    found.sort();
    found
}

#[test]
fn dry_run_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(bench(tmp.path(), &[]), 0);
    let found = results(tmp.path());
    assert_eq!(found.len(), 1);
    assert!(found[0].file_name().unwrap().to_string_lossy().ends_with("-dry-run.json"));
    let result: Value = serde_json::from_str(&std::fs::read_to_string(&found[0]).unwrap()).unwrap();
    assert_eq!(result["dry_run"], Value::Bool(true));
    let runs = result["cases"][0]["runs"].as_array().unwrap();
    assert_eq!(runs.iter().map(|r| r["seed"].as_i64().unwrap()).collect::<Vec<_>>(), [7, 8]);
    assert!(runs.iter().all(|r| r.get("scores").is_some() && r.get("error").is_none()), "{runs:?}");
    assert!(runs.iter().all(|r| r["crowd_rules"] == serde_json::json!(tellfold_models::CROWD_RULES)));
    assert_eq!(result["summary"]["all"]["graded"], 2);
    assert_eq!(result["zero_shot_summary"]["all"]["graded"], 1);
    assert_eq!(result["crowd_rules"], serde_json::json!([tellfold_models::CROWD_RULES]));
    let md = std::fs::read_to_string(found[0].with_extension("md")).unwrap();
    assert!(md.contains("Dry run"));

    // resuming the same pass keeps the finished runs, their grades and the saved zero-shot answer
    let pass = tmp.path().join("runs").join(found[0].file_stem().unwrap());
    let grade = pass.join("corner-cafe-prices/seed-7/grade.json");
    let before = std::fs::metadata(&grade).unwrap().modified().unwrap();
    assert_eq!(bench(tmp.path(), &["--resume", pass.to_str().unwrap()]), 0);
    assert_eq!(std::fs::metadata(&grade).unwrap().modified().unwrap(), before);
    let found = results(tmp.path());
    assert_eq!(found.len(), 2, "a second results file next to the first");
    let resumed: Value = serde_json::from_str(&std::fs::read_to_string(&found[1]).unwrap()).unwrap();
    assert_eq!(resumed["cases"], result["cases"]);
}

