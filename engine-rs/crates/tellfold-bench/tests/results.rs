//! The committed results in `bench/results` read back unchanged, and their Markdown summaries come out of them again.

use tellfold_bench::output::{markdown, result_text};

fn results() -> Vec<std::path::PathBuf> {
    let mut found: Vec<_> = std::fs::read_dir(tellfold_bench::results_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    found.sort();
    assert!(!found.is_empty(), "bench/results has no results");
    found
}

#[test]
fn every_result_reads_and_writes_back_byte_for_byte() {
    for path in results() {
        let text = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(result_text(&value) == text, "{} changes when written back", path.display());
    }
}

/// Summaries written before the case table gained its "planner's mix" column (crowd rules 2), so the harness no
/// longer writes them as they are.
const OLDER_TABLE: &[&str] = &["2026-11-11-sonnet-haiku.md", "2026-11-12-sonnet-haiku.md"];

#[test]
fn every_markdown_summary_comes_out_of_its_result() {
    for path in results() {
        if OLDER_TABLE.iter().any(|name| path.with_extension("md").ends_with(name)) {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let md = std::fs::read_to_string(path.with_extension("md")).unwrap();
        let ours = markdown(&value);
        if ours != md {
            let first = ours.lines().zip(md.lines()).position(|(a, b)| a != b);
            panic!(
                "{}: first differing line {:?}\nours:  {:?}\nfile:  {:?}",
                path.display(),
                first,
                first.map(|i| ours.lines().nth(i)),
                first.map(|i| md.lines().nth(i))
            );
        }
    }
}
