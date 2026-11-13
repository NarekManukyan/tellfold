//! The results files: `<date>-<label>.json` and its Markdown summary, and merging the parts of a sharded pass.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use tellfold_models::json::{py_dumps_ascii, py_dumps_pretty, py_str, py_sum};

use crate::cases::load_cases;
use crate::process::head;
use crate::score::{baselines, count_failures, crowd_rules_warned, planner_vs_crowd, summarise};
use crate::truthy;

/// A share as a whole percentage, or an en dash for none.
pub fn pct(value: &Value) -> String {
    match value.as_f64() {
        Some(v) => format!("{:.0}%", v * 100.0),
        None => "–".into(),
    }
}

fn get<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k).unwrap_or(&Value::Null)
}

/// `d.get(k, default)` printed with `str()`: the default only when the key is missing.
fn str_or(v: &Value, k: &str, default: &str) -> String {
    v.get(k).map_or_else(|| default.to_string(), py_str)
}

/// The Markdown summary of a result.
pub fn markdown(result: &Value) -> String {
    let s = get(result, "summary");
    let b = get(result, "baselines");
    let settings = get(result, "settings");
    let cases = get(result, "cases").as_array().map_or(&[][..], Vec::as_slice);
    let mut lines: Vec<String> = vec![
        format!("# Tellfold accuracy: {}", py_str(get(result, "label"))),
        String::new(),
        format!(
            "Date {} · commit `{}` · {} cases × {} run(s) · agents {}, planner {}, judge {}",
            py_str(get(result, "date")),
            py_str(get(result, "git_commit")),
            cases.len(),
            py_str(get(result, "repeats")),
            py_str(get(settings, "agent_model")),
            py_str(get(settings, "planner_model")),
            py_str(get(get(result, "judge"), "model")),
        ),
        String::new(),
    ];
    if truthy(get(result, "dry_run")) {
        lines.push(
            "**Dry run.** The simulation used the CLI's offline model and the judge was a word-overlap stand-in. \
             These numbers test the harness and say nothing about accuracy."
                .into(),
        );
        lines.push(String::new());
    }
    lines.push("| | graded | direction | objection recall | backdown | groups named | direction from sentiment |".into());
    lines.push("|---|---|---|---|---|---|---|".into());
    for (name, label) in
        [("all", "all cases"), ("after_cutoff", "after the model's cutoff"), ("known_to_model", "known to the model")]
    {
        let x = get(s, name);
        lines.push(format!(
            "| {label} | {} | {} | {} | {} | {} | {} |",
            py_str(get(x, "graded")),
            pct(get(x, "direction_accuracy")),
            pct(get(x, "objection_recall")),
            pct(get(x, "backdown_accuracy")),
            pct(get(x, "group_recall")),
            pct(get(x, "sentiment_direction_accuracy")),
        ));
    }
    if truthy(get(result, "zero_shot_summary")) {
        let x = get(get(result, "zero_shot_summary"), "all");
        lines.push(format!(
            "| zero-shot baseline | {} | {} | {} | {} | {} | – |",
            py_str(get(x, "graded")),
            pct(get(x, "direction_accuracy")),
            pct(get(x, "objection_recall")),
            pct(get(x, "backdown_accuracy")),
            pct(get(x, "group_recall")),
        ));
    }
    let pc = get(result, "planner_vs_crowd");
    if truthy(pc) {
        lines.push(format!(
            "| planner's stance mix alone | {} | {} | – | – | – | – |",
            py_str(get(pc, "runs")),
            pct(get(pc, "planner_direction_accuracy"))
        ));
        lines.push(String::new());
        lines.push(format!(
            "The crowd's final direction moved away from the planner's mix in {} of {} runs: {} to the right answer, {} \
             away from it, {} from one wrong answer to another. Tellfold agrees with the zero-shot answer on direction \
             in {} of runs; the planner's mix alone agrees with it in {}.",
            py_str(get(pc, "moved")),
            py_str(get(pc, "runs")),
            py_str(get(pc, "moved_right")),
            py_str(get(pc, "moved_wrong")),
            py_str(get(pc, "moved_neither")),
            pct(get(pc, "agrees_with_zero_shot")),
            pct(get(pc, "planner_agrees_with_zero_shot")),
        ));
    }
    if truthy(b) {
        lines.push(String::new());
        lines.push(format!(
            "Ignoring the announcement: always \"negative\" gets the direction right {} of the time; always \"no \
             backdown\" gets the backdown right {}.",
            pct(get(b, "always_negative_direction")),
            pct(get(b, "always_no_backdown"))
        ));
    }
    let (failed_runs, failed_zero) = count_failures(cases);
    if failed_runs > 0 || failed_zero > 0 {
        lines.push(String::new());
        lines.push(format!(
            "**Failures:** {failed_runs} simulation run(s) and {failed_zero} zero-shot answer(s) failed; the case table \
             shows each error. Failed entries are left out of the averages."
        ));
    }
    lines.extend(
        [
            "",
            "## Cases",
            "",
            "| case | actual | predicted | planner's mix | objections | backdown (actual / predicted) | zero-shot | error |",
            "|---|---|---|---|---|---|---|---|",
        ]
        .map(String::from),
    );
    for c in cases {
        let r = get(c, "rubric");
        let zero = zero_shot_cell(get(c, "zero_shot"));
        for run in get(c, "runs").as_array().into_iter().flatten() {
            let sc = get(run, "scores");
            let grade = if truthy(get(run, "grade")) { get(run, "grade").clone() } else { json!({}) };
            let pred = str_or(&grade, "predicted_direction", "–");
            let recall = if truthy(sc) { pct(get(sc, "objection_recall")) } else { "–".into() };
            let mark = if !truthy(sc) {
                ""
            } else if truthy(get(sc, "direction_match")) {
                " ✓"
            } else {
                " ✗"
            };
            let backdown_predicted = if sc.is_object() { str_or(sc, "backdown_predicted", "–") } else { "–".into() };
            lines.push(format!(
                "| {} (seed {}) | {} | {pred}{mark} | {} | {recall} | {} / {backdown_predicted} | {zero} | {} |",
                py_str(get(c, "slug")),
                py_str(get(run, "seed")),
                py_str(get(r, "direction")),
                str_or(run, "planner_direction", "–"),
                py_str(get(get(r, "backdown"), "kind")),
                head(&str_or(run, "error", ""), 80),
            ));
        }
    }
    lines.push(String::new());
    lines.push(format!("Usage: {}", py_dumps_ascii(get(result, "usage"))));
    lines.push(String::new());
    lines.join("\n")
}

/// One case's zero-shot answer for the case table: direction, recall and backdown, or its error.
fn zero_shot_cell(zs: &Value) -> String {
    if !truthy(zs) {
        return String::new();
    }
    if truthy(get(zs, "error")) {
        return format!("failed: {}", head(&py_str(get(zs, "error")), 60));
    }
    let sc = get(zs, "scores");
    if !truthy(sc) {
        return "–".into();
    }
    let grade = if truthy(get(zs, "grade")) { get(zs, "grade").clone() } else { json!({}) };
    format!(
        "{}{}, {}, {}",
        str_or(&grade, "predicted_direction", "–"),
        if truthy(get(sc, "direction_match")) { " ✓" } else { " ✗" },
        pct(get(sc, "objection_recall")),
        py_str(get(sc, "backdown_predicted")),
    )
}

pub fn write_markdown(path: &Path, result: &Value) -> Result<(), String> {
    std::fs::write(path, markdown(result)).map_err(|e| format!("{}: {e}", path.display()))
}

/// A result as `json.dumps(result, indent=2, ensure_ascii=False)` writes it, with a final newline.
pub fn result_text(result: &Value) -> String {
    py_dumps_pretty(result) + "\n"
}

/// `<date>-<label>.json`, or `<date>-<label>-2.json` and up when that exists. Labels may contain dots.
pub fn results_path(folder: &Path, label: &str, date: &str) -> PathBuf {
    let mut path = folder.join(format!("{date}-{label}.json"));
    let mut n = 2;
    while path.exists() {
        path = folder.join(format!("{date}-{label}-{n}.json"));
        n += 1;
    }
    path
}

/// The Markdown file next to a results JSON.
pub fn markdown_path(json_path: &Path) -> PathBuf {
    json_path.with_extension("md")
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Adds up one usage block (`judge` or `zero_shot`) over the parts that have it, key by key.
fn total_usage(parts: &[Value], key: &str) -> Value {
    let usages: Vec<&Value> = parts.iter().map(|p| get(get(p, "usage"), key)).filter(|u| truthy(u)).collect();
    let Some(first) = usages.first() else { return Value::Null };
    let mut out = Map::new();
    for k in first.as_object().into_iter().flatten().map(|(k, _)| k) {
        let values: Vec<&Value> = usages.iter().map(|u| get(u, k)).collect();
        let total = if values.iter().all(|v| v.is_i64() || v.is_u64()) {
            json!(values.iter().filter_map(|v| v.as_i64()).sum::<i64>())
        } else {
            json!(py_sum(values.iter().map(|v| v.as_f64().unwrap_or(0.0))))
        };
        out.insert(k.clone(), total);
    }
    Value::Object(out)
}

/// One result from the parts of a pass run in parallel with `--cases`: the cases in case order, summaries and
/// baselines computed again, usage added up. Every part must come from the same commit, settings and repeats.
/// Returns the exit code.
pub fn merge_results(paths: &[PathBuf], results_dir: &Path, cases_dir: &Path) -> Result<i32, String> {
    let parts: Vec<Value> = paths.iter().map(|p| read_json(p)).collect::<Result<_, _>>()?;
    let Some(first) = parts.first() else { return Err("--merge needs at least one result".into()) };
    let same = ["git_commit", "settings", "repeats", "dry_run", "tellfold", "judge"];
    for (path, part) in paths.iter().zip(&parts) {
        let differ: Vec<&str> = same.iter().copied().filter(|k| part.get(*k) != first.get(*k)).collect();
        if !differ.is_empty() {
            eprintln!("{} differs from {} in {}", path.display(), paths[0].display(), differ.join(", "));
            return Ok(1);
        }
    }
    let order: Vec<String> = load_cases(None, cases_dir)?.into_iter().map(|c| c.slug).collect();
    let rank = |c: &Value| {
        let slug = get(c, "slug").as_str().unwrap_or("");
        order.iter().position(|s| s == slug).unwrap_or(order.len())
    };
    let mut cases: Vec<Value> =
        parts.iter().flat_map(|p| get(p, "cases").as_array().cloned().unwrap_or_default()).collect();
    cases.sort_by_key(|c| rank(c)); // stable, like sorted()
    let slugs: Vec<&Value> = cases.iter().map(|c| get(c, "slug")).collect();
    if slugs.iter().enumerate().any(|(i, s)| slugs[..i].contains(s)) {
        eprintln!("a case is in more than one part");
        return Ok(1);
    }
    let zero = cases.iter().any(|c| truthy(get(c, "zero_shot")));
    let simulation_calls: i64 =
        parts.iter().map(|p| get(get(p, "usage"), "simulation_calls").as_i64().unwrap_or(0)).sum();
    let mut result = first.clone();
    let fields = [
        ("crowd_rules", json!(crowd_rules_warned(&cases))),
        ("summary", summarise(&cases, "runs")),
        ("zero_shot_summary", if zero { summarise(&cases, "zero_shot") } else { Value::Null }),
        ("baselines", baselines(&cases)),
        ("planner_vs_crowd", planner_vs_crowd(&cases)),
        (
            "usage",
            json!({"simulation_calls": simulation_calls, "judge": total_usage(&parts, "judge"),
                   "zero_shot": total_usage(&parts, "zero_shot")}),
        ),
    ];
    let count = cases.len();
    let object = result.as_object_mut().ok_or("a result is not an object")?;
    for (k, v) in fields {
        object.insert(k.into(), v);
    }
    object.insert("cases".into(), Value::Array(cases));
    let out_json =
        results_path(results_dir, &py_str(get(first, "label")), &py_str(get(first, "date")));
    std::fs::write(&out_json, result_text(&result)).map_err(|e| format!("{}: {e}", out_json.display()))?;
    let md = markdown_path(&out_json);
    write_markdown(&md, &result)?;
    println!(
        "wrote {} and {} from {} parts, {count} cases",
        out_json.display(),
        md.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        parts.len()
    );
    Ok(0)
}
