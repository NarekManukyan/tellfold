//! The model calls and API-equivalent cost of a pass, by purpose (`--estimate`).

use serde_json::{json, Map, Value};
use tellfold_models::json::{py_round, py_round_int, py_sum};
use tellfold_models::STAKE_RULES;
use tellfold_store::estimate::{crowd_rules_calls, CHUNKS_PER_CALL, ESTIMATED_ACTIVE_SHARE, MAX_EXTRACT_CHUNKS};

use crate::cases::Case;
use crate::Settings;

/// Average characters (prompt, output) per call by purpose, measured from five real 20-agent, 6-round runs (Claude
/// Code, haiku agents, sonnet planner, 7-11 Nov 2026; `calls.jsonl` records prompt and output lengths only). Seeds in
/// this benchmark are 200-500 words, shorter than those runs' seeds, so graph and report prompts are rounded down a
/// little.
pub const CALL_CHARS: &[(&str, (i64, i64))] = &[
    ("graph.extract", (5_000, 8_000)),
    ("personas.batch", (10_000, 8_000)),
    ("engine.seed", (10_500, 2_100)),
    ("engine.round", (27_000, 3_300)),
    ("engine.sentiment", (2_000, 160)),
    ("report.write", (21_000, 10_500)),
    // crowd rules 2 and later, from the 129 runs of the fourth pass (13 Nov 2026)
    ("personas.stakes", (4_800, 2_000)),
    ("engine.factcheck", (3_300, 20)),
    ("judge", (16_000, 2_000)),
    ("zero_shot", (4_000, 5_000)),
];
pub const CHARS_PER_TOKEN: i64 = 4;
/// System prompt and JSON schema sent with every call, which `calls.jsonl` does not count.
pub const OVERHEAD_TOKENS: i64 = 1_500;
/// The report call keeps extended thinking on; hidden thinking tokens are billed as output.
pub const REPORT_THINKING_TOKENS: i64 = 8_000;
/// USD per million tokens (input, output), API list prices for prompts under 100k tokens, Nov 2026. A Claude Code
/// subscription is not billed per token; this is the API-equivalent cost, to compare against plan limits.
pub const PRICES: &[(&str, (f64, f64))] =
    &[("haiku", (0.10, 0.50)), ("sonnet", (2.00, 10.00)), ("opus", (5.00, 25.00))];
pub const CHUNK_SIZE: i64 = 1800;
pub const CHUNK_OVERLAP: i64 = 200;

fn ceil_div(a: i64, b: i64) -> i64 {
    (a as f64 / b as f64).ceil() as i64
}

/// Model calls by purpose for one run, the formula of `tellfold_store::estimate::estimate_call_split` split by
/// purpose. From crowd rules 2 a run also plans its stakes and fact-checks organisation posts in the opening and each
/// round; those counts come from [`crowd_rules_calls`] itself.
pub fn estimate_calls(agents: i64, rounds: i64, batch: i64, chunks: i64, crowd_rules: i64) -> Vec<(&'static str, i64)> {
    let active = 1.max((agents as f64 * ESTIMATED_ACTIVE_SHARE).ceil() as i64);
    let mut calls = vec![
        ("graph.extract", ceil_div(chunks.min(MAX_EXTRACT_CHUNKS), CHUNKS_PER_CALL)),
        ("personas.batch", ceil_div(agents, 10)),
        ("engine.seed", ceil_div(py_round_int(0.3 * agents as f64), batch)),
        ("engine.round", rounds * ceil_div(active, batch)),
        ("engine.sentiment", rounds + 1),
        ("report.write", 1),
    ];
    if crowd_rules >= STAKE_RULES {
        let checks = rounds + 1; // the opening posts are checked too
        let stakes = crowd_rules_calls(crowd_rules, false, checks) - crowd_rules_calls(crowd_rules, true, checks);
        calls.push(("personas.stakes", stakes));
        calls.push(("engine.factcheck", crowd_rules_calls(crowd_rules, true, checks)));
    }
    calls
}

/// The chunks the engine cuts a seed into, as a character count (no paragraph packing).
pub fn chunk_count(text: &str) -> i64 {
    let len = text.chars().count() as i64;
    1.max(ceil_div(len - CHUNK_OVERLAP, CHUNK_SIZE - CHUNK_OVERLAP))
}

/// (tokens in, tokens out, USD) for `calls` calls of one purpose.
pub fn call_cost(purpose: &str, model: &str, calls: i64) -> Result<(i64, i64, f64), String> {
    let (chars_in, chars_out) = CALL_CHARS.iter().find(|(p, _)| *p == purpose).map(|(_, c)| *c).expect("known purpose");
    let tokens_in = calls * (chars_in / CHARS_PER_TOKEN + OVERHEAD_TOKENS);
    let thinking = if purpose == "report.write" { REPORT_THINKING_TOKENS } else { 0 };
    let tokens_out = calls * (chars_out / CHARS_PER_TOKEN + thinking);
    let (price_in, price_out) = price_for(model)?;
    Ok((tokens_in, tokens_out, (tokens_in as f64 * price_in + tokens_out as f64 * price_out) / 1e6))
}

/// List price for a model alias or full id ("sonnet", "claude-sonnet-5-5"); an unknown model is an error.
pub fn price_for(model: &str) -> Result<(f64, f64), String> {
    let lower = model.to_lowercase();
    PRICES.iter().find(|(family, _)| lower.contains(family)).map(|(_, price)| *price).ok_or_else(|| {
        let families: Vec<&str> = PRICES.iter().map(|(f, _)| *f).collect();
        format!(
            "no price for model {}: use one of {} or a full id containing one",
            tellfold_models::json::py_repr(&Value::String(model.into())),
            families.join(", ")
        )
    })
}

/// Calls, tokens and cost of a pass: one row per purpose in the order they first come up, and the total.
pub fn estimate(
    cases: &[Case],
    settings: &Settings,
    repeats: i64,
    zero_shot: bool,
    judge_model: &str,
) -> Result<Value, String> {
    // purpose -> (model, calls, tokens in, tokens out, usd)
    let mut rows: Vec<(&str, String, i64, i64, i64, f64)> = Vec::new();
    let mut add = |purpose: &'static str, model: &str, calls: i64| -> Result<(), String> {
        let (tokens_in, tokens_out, usd) = call_cost(purpose, model, calls)?;
        let at = match rows.iter().position(|r| r.0 == purpose) {
            Some(at) => at,
            None => {
                rows.push((purpose, model.to_string(), 0, 0, 0, 0.0));
                rows.len() - 1
            }
        };
        let row = &mut rows[at];
        row.2 += calls;
        row.3 += tokens_in;
        row.4 += tokens_out;
        row.5 += usd;
        Ok(())
    };
    for case in cases {
        let text = std::fs::read_to_string(&case.input).map_err(|e| format!("{}: {e}", case.input.display()))?;
        let calls = estimate_calls(
            settings.agents,
            settings.rounds,
            settings.batch,
            chunk_count(&text),
            tellfold_models::CROWD_RULES,
        );
        for _ in 0..repeats {
            for &(purpose, n) in &calls {
                let agent = purpose.starts_with("engine.") && purpose != "engine.factcheck";
                add(purpose, if agent { &settings.agent_model } else { &settings.planner_model }, n)?;
            }
            add("judge", judge_model, 1)?;
        }
        if zero_shot {
            add("zero_shot", &settings.planner_model, 1)?;
            add("judge", judge_model, 1)?;
        }
    }
    let mut out_rows = Map::new();
    for (purpose, model, calls, tokens_in, tokens_out, usd) in &rows {
        out_rows.insert(
            purpose.to_string(),
            json!({"model": model, "calls": calls, "tokens_in": tokens_in, "tokens_out": tokens_out,
                   "usd": py_round(*usd, 3)}),
        );
    }
    Ok(json!({
        "cases": cases.len(),
        "repeats": repeats,
        "zero_shot": zero_shot,
        "rows": out_rows,
        "total": {
            "calls": rows.iter().map(|r| r.2).sum::<i64>(),
            "tokens_in": rows.iter().map(|r| r.3).sum::<i64>(),
            "tokens_out": rows.iter().map(|r| r.4).sum::<i64>(),
            "usd": py_round(py_sum(rows.iter().map(|r| r.5)), 2),
        },
    }))
}

/// `1234567` as `1,234,567`.
fn grouped(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// The estimate as the table `--estimate` prints.
pub fn format_estimate(est: &Value) -> String {
    let int = |v: &Value| v.as_i64().unwrap_or(0);
    let mut out = format!(
        "{} cases x {} run(s){}\n",
        int(&est["cases"]),
        int(&est["repeats"]),
        if est["zero_shot"].as_bool() == Some(true) { " + zero-shot baseline" } else { "" }
    );
    out +=
        &format!("{:<18}{:<8}{:>7}{:>12}{:>12}{:>9}\n", "purpose", "model", "calls", "tokens in", "tokens out", "USD");
    for (purpose, r) in est["rows"].as_object().into_iter().flatten() {
        out += &format!(
            "{:<18}{:<8}{:>7}{:>12}{:>12}{:>9.2}\n",
            purpose,
            r["model"].as_str().unwrap_or(""),
            int(&r["calls"]),
            grouped(int(&r["tokens_in"])),
            grouped(int(&r["tokens_out"])),
            r["usd"].as_f64().unwrap_or(0.0)
        );
    }
    let t = &est["total"];
    out += &format!(
        "{:<26}{:>7}{:>12}{:>12}{:>9.2}\n",
        "total",
        int(&t["calls"]),
        grouped(int(&t["tokens_in"])),
        grouped(int(&t["tokens_out"])),
        t["usd"].as_f64().unwrap_or(0.0)
    );
    out + "API-equivalent list prices; on a Claude Code plan this counts against usage limits instead.\n"
}
