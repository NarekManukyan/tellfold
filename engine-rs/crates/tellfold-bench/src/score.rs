//! Scoring a grade against its rubric, and the averages, baselines and planner comparison of a pass.

use serde_json::{json, Map, Value};
use tellfold_models::json::{py_round, py_str, py_sum};

use crate::{truthy, DIRECTIONS};

fn verdict_points(v: &Value) -> f64 {
    match v.as_str() {
        Some("covered") => 1.0,
        Some("partial") => 0.5,
        _ => 0.0,
    }
}

/// Ids and group names compared ignoring case and spacing (`str(value or "")`, lowercased, whitespace collapsed).
fn key(value: &Value) -> String {
    let text = if truthy(value) { py_str(value) } else { String::new() };
    text.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn get<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k).unwrap_or(&Value::Null)
}

fn list(v: &Value) -> &[Value] {
    v.as_array().map_or(&[], Vec::as_slice)
}

fn direction_index(v: &Value) -> Option<usize> {
    v.as_str().and_then(|s| DIRECTIONS.iter().position(|d| *d == s))
}

/// Compares the judge's blind classification with the rubric. Ids and group names are matched ignoring case and
/// spacing; anything the judge returned that matches nothing in the rubric is listed in `unmatched`.
pub fn score(rubric: &Value, grade: &Value) -> Result<Value, String> {
    let mut by_id: Map<String, Value> = Map::new();
    for o in list(get(grade, "objections")) {
        by_id.insert(key(get(o, "id")), o.clone());
    }
    let verdicts: Vec<Value> = list(get(rubric, "objections"))
        .iter()
        .map(|o| match by_id.get(&key(get(o, "id"))) {
            Some(found) => found.get("verdict").cloned().unwrap_or(json!("missed")),
            None => json!("missed"),
        })
        .collect();
    if verdicts.is_empty() {
        return Err("division by zero".into());
    }
    let recall = py_sum(verdicts.iter().map(verdict_points)) / verdicts.len() as f64;
    let wanted = list(get(rubric, "pushback_groups"));
    let mut named: Map<String, Value> = Map::new();
    for g in list(get(grade, "groups")) {
        named.insert(key(get(g, "group")), Value::Bool(truthy(get(g, "named"))));
    }
    let group_recall = (!wanted.is_empty()).then(|| {
        let hits = wanted.iter().filter(|g| named.get(&key(g)) == Some(&Value::Bool(true))).count();
        hits as f64 / wanted.len() as f64
    });
    let known: Vec<String> =
        list(get(rubric, "objections")).iter().map(|o| key(get(o, "id"))).chain(wanted.iter().map(key)).collect();
    let unmatched: Vec<&String> = by_id.keys().chain(named.keys()).filter(|k| !known.contains(k)).collect();
    let predicted = get(grade, "predicted_direction");
    let actual = get(rubric, "direction");
    let distance = match (direction_index(predicted), direction_index(actual)) {
        (Some(p), Some(a)) => json!(p.abs_diff(a)),
        (Some(_), None) => return Err(format!("{} is not in list", tellfold_models::json::py_repr(actual))),
        _ => Value::Null,
    };
    let backdown = get(grade, "predicted_backdown");
    let changes = |v: &Value| matches!(v.as_str(), Some("partial" | "full"));
    let actual_kind = get(get(rubric, "backdown"), "kind");
    Ok(json!({
        "direction_match": predicted == actual,
        "direction_distance": distance,
        "objection_recall": py_round(recall, 3),
        "objection_verdicts": verdicts,
        "backdown_predicted": backdown,
        // a report that does not say what the organisation will do gets no credit either way
        "backdown_match": backdown.as_str() != Some("unclear") && changes(backdown) == changes(actual_kind),
        "backdown_kind_match": backdown == actual_kind,
        "group_recall": group_recall.map(|g| py_round(g, 3)),
        "unmatched": unmatched,
    }))
}

/// The mean of the values that are not null, to three places; null when there are none.
pub fn mean(values: impl IntoIterator<Item = Option<f64>>) -> Value {
    let values: Vec<f64> = values.into_iter().flatten().collect();
    if values.is_empty() {
        return Value::Null;
    }
    json!(py_round(py_sum(values.iter().copied()) / values.len() as f64, 3))
}

fn as_f64(v: &Value) -> Option<f64> {
    v.as_f64()
}

fn as_flag(v: &Value) -> Option<f64> {
    Some(if truthy(v) { 1.0 } else { 0.0 })
}

/// A case's simulation runs (`runs`, a list) or its zero-shot answer (`zero_shot`, one entry), when graded.
pub fn graded_entries<'a>(case: &'a Value, key: &str) -> Vec<&'a Value> {
    let entries = get(case, key);
    let entries: Vec<&Value> = match entries {
        Value::Object(_) => vec![entries],
        Value::Array(items) => items.iter().collect(),
        _ => Vec::new(),
    };
    entries.into_iter().filter(|e| truthy(get(e, "scores"))).collect()
}

fn block(selected: &[&Value], key: &str) -> Value {
    let scores: Vec<&Value> = selected.iter().flat_map(|c| graded_entries(c, key)).map(|e| get(e, "scores")).collect();
    let field = |name: &str, f: fn(&Value) -> Option<f64>| mean(scores.iter().map(|s| f(get(s, name))));
    let sentiment = selected.iter().flat_map(|c| {
        let actual = get(get(c, "rubric"), "direction");
        graded_entries(c, key)
            .into_iter()
            .filter(|e| truthy(get(e, "sentiment_direction")))
            .map(move |e| Some(if get(e, "sentiment_direction") == actual { 1.0 } else { 0.0 }))
    });
    json!({
        "graded": scores.len(),
        "direction_accuracy": field("direction_match", as_flag),
        "objection_recall": field("objection_recall", as_f64),
        "backdown_accuracy": field("backdown_match", as_flag),
        "group_recall": field("group_recall", as_f64),
        "sentiment_direction_accuracy": mean(sentiment),
    })
}

/// Averages over every graded run (or zero-shot answer), overall and split by whether the model could know.
pub fn summarise(cases: &[Value], key: &str) -> Value {
    let all: Vec<&Value> = cases.iter().collect();
    let known = |c: &&Value| truthy(get(get(c, "rubric"), "known_to_model"));
    let after: Vec<&Value> = cases.iter().filter(|c| !known(c)).collect();
    let before: Vec<&Value> = cases.iter().filter(known).collect();
    json!({"all": block(&all, key), "after_cutoff": block(&after, key), "known_to_model": block(&before, key)})
}

/// What a forecaster that ignores the announcement would score: the most common answer every time.
pub fn baselines(cases: &[Value]) -> Value {
    let n = cases.len();
    if n == 0 {
        return json!({});
    }
    let directions: Vec<&Value> = cases.iter().map(|c| get(get(c, "rubric"), "direction")).collect();
    let count = |d: &str| directions.iter().filter(|x| x.as_str() == Some(d)).count();
    // the first of the most common, as Python's max() picks
    let majority =
        DIRECTIONS.iter().copied().fold(DIRECTIONS[0], |best, d| if count(d) > count(best) { d } else { best });
    let changed =
        cases.iter().filter(|c| get(get(get(c, "rubric"), "backdown"), "kind").as_str() != Some("none")).count();
    let share = |k: usize| py_round(k as f64 / n as f64, 3);
    json!({
        "always_negative_direction": share(count("negative")),
        "majority_direction": majority,
        "majority_direction_accuracy": share(count(majority)),
        "always_no_backdown": share(n - changed),
        "always_backdown": share(changed),
    })
}

/// The direction the planner's own stance mix implies (crowd rules 2), by the engine's first-reaction rule
/// (`direction_of` in tellfold-report): mixed unless at least half the people take a side and 3 in 5 of those take
/// the same one.
pub fn mix_direction(mix: &Value) -> &'static str {
    let share = |k: &str| mix.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let (s, c) = (share("supportive"), share("critical"));
    let total = s + c + share("undecided") + share("indifferent");
    if total <= 0.0 || s + c < 0.5 * total {
        return "mixed";
    }
    if c >= 0.6 * (s + c) {
        return "negative";
    }
    if s >= 0.6 * (s + c) {
        return "positive";
    }
    "mixed"
}

/// The planner's own mix scored as a forecast, and how often the crowd's final direction moved away from it: the
/// question whether the crowd adds anything to the planner's guess.
pub fn planner_vs_crowd(cases: &[Value]) -> Value {
    let runs: Vec<(&Value, &Value)> = cases
        .iter()
        .flat_map(|c| list(get(c, "runs")).iter().map(move |r| (c, r)))
        .filter(|(_, r)| truthy(get(r, "planner_direction")) && truthy(get(r, "scores")))
        .collect();
    if runs.is_empty() {
        return Value::Null;
    }
    let actual = |c: &Value| get(get(c, "rubric"), "direction").clone();
    let crowd = |r: &Value| get(get(r, "grade"), "predicted_direction").clone();
    let planner = |r: &Value| get(r, "planner_direction").clone();
    let flag = |b: bool| Some(if b { 1.0 } else { 0.0 });
    let moved: Vec<&(&Value, &Value)> = runs.iter().filter(|(_, r)| crowd(r) != planner(r)).collect();
    let zero: Vec<&(&Value, &Value)> = runs.iter().filter(|(c, _)| truthy(get(get(c, "zero_shot"), "grade"))).collect();
    let zero_direction = |c: &Value| get(get(get(c, "zero_shot"), "grade"), "predicted_direction").clone();
    json!({
        "runs": runs.len(),
        "planner_direction_accuracy": mean(runs.iter().map(|(c, r)| flag(planner(r) == actual(c)))),
        "crowd_direction_accuracy": mean(runs.iter().map(|(c, r)| flag(crowd(r) == actual(c)))),
        "moved": moved.len(),
        "moved_right": moved.iter().filter(|(c, r)| crowd(r) == actual(c)).count(),
        "moved_wrong": moved.iter().filter(|(c, r)| planner(r) == actual(c)).count(),
        "moved_neither": moved.iter().filter(|(c, r)| actual(c) != crowd(r) && actual(c) != planner(r)).count(),
        "agrees_with_zero_shot": mean(zero.iter().map(|(c, r)| flag(crowd(r) == zero_direction(c)))),
        "planner_agrees_with_zero_shot": mean(zero.iter().map(|(c, r)| flag(planner(r) == zero_direction(c)))),
    })
}

/// The crowd rules versions the runs of a pass followed, warning when there is more than one: a resumed pass whose
/// working copy moved on mixes versions (five runs of the version 2 balanced pass ran version 3).
pub fn crowd_rules_of(cases: &[Value]) -> (Vec<i64>, Option<String>) {
    let mut versions: Vec<i64> = cases
        .iter()
        .flat_map(|c| list(get(c, "runs")))
        .filter_map(|r| r.get("crowd_rules").and_then(Value::as_i64))
        .collect();
    versions.sort_unstable();
    versions.dedup();
    let warning = (versions.len() > 1).then(|| {
        let shown: Vec<String> = versions.iter().map(i64::to_string).collect();
        format!("warning: the runs follow more than one crowd rules version: [{}]", shown.join(", "))
    });
    (versions, warning)
}

/// [`crowd_rules_of`], printing its warning to stderr.
pub fn crowd_rules_warned(cases: &[Value]) -> Vec<i64> {
    let (versions, warning) = crowd_rules_of(cases);
    if let Some(warning) = warning {
        eprintln!("{warning}");
    }
    versions
}

/// (failed simulation runs, failed zero-shot answers).
pub fn count_failures(cases: &[Value]) -> (usize, usize) {
    let runs = cases.iter().flat_map(|c| list(get(c, "runs"))).filter(|r| truthy(get(r, "error"))).count();
    let zero = cases.iter().filter(|c| truthy(get(get(c, "zero_shot"), "error"))).count();
    (runs, zero)
}
