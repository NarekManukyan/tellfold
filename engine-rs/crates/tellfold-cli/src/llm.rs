//! Which model backend a command talks to: the run's runtimes, or the canned offline model when
//! `TELLFOLD_FAKE_LLM=1` (the `make_llm` part of `tellfold/cli.py`).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use tellfold_llm::canned::fake_responder;
use tellfold_llm::fake::FakeLlm;
use tellfold_llm::{Llm, LlmError};
use tellfold_models::SimConfig;
use tellfold_runtimes::settings::env;
use tellfold_runtimes::{build_llm, known_ids, load_settings, LlmOptions};

pub const FAKE_ENV: &str = "TELLFOLD_FAKE_LLM";
pub const FAKE_DELAY_ENV: &str = "TELLFOLD_FAKE_DELAY";

pub fn fake_mode() -> bool {
    let value = env(FAKE_ENV).unwrap_or_default();
    matches!(tellfold_models::text::py_strip(&value).to_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

/// Seconds each fake model call waits (TELLFOLD_FAKE_DELAY, 0-10, default 0), so a fake run can be watched live.
pub fn fake_delay() -> f64 {
    let raw = env(FAKE_DELAY_ENV).unwrap_or_default();
    delay_seconds(if raw.is_empty() { "0" } else { &raw })
}

/// `float(text)` clamped to 0-10, or 0 where Python's `float()` raises.
fn delay_seconds(raw: &str) -> f64 {
    // float(): surrounding whitespace is allowed, and `_` only between two digits
    let text = tellfold_models::text::py_strip(raw);
    let chars: Vec<char> = text.chars().collect();
    let digit_group = |i: usize| {
        i > 0
            && chars.get(i - 1).is_some_and(char::is_ascii_digit)
            && chars.get(i + 1).is_some_and(char::is_ascii_digit)
    };
    if chars.iter().enumerate().any(|(i, c)| *c == '_' && !digit_group(i)) {
        return 0.0;
    }
    match text.replace('_', "").parse::<f64>() {
        Ok(v) if !v.is_nan() => v.clamp(0.0, 10.0),
        _ => 0.0,
    }
}

/// The run's model backend, or the canned fake when `TELLFOLD_FAKE_LLM=1`.
///
/// The agent / planner runtimes come from `config`, else from `run_dir/config.json`, else the settings.
pub fn make_llm(
    run_dir: Option<&Path>,
    concurrency: i64,
    think: bool,
    config: Option<&SimConfig>,
) -> Result<Arc<dyn Llm>, LlmError> {
    if fake_mode() {
        let delay = fake_delay();
        if delay <= 0.0 {
            return Ok(Arc::new(FakeLlm::new(fake_responder)));
        }
        return Ok(Arc::new(FakeLlm::new_async(move |purpose, prompt, schema| {
            let reply = fake_responder(purpose, prompt, schema);
            async move {
                tokio::time::sleep(Duration::from_secs_f64(delay)).await;
                reply
            }
            .boxed()
        })));
    }
    let settings = load_settings(None);
    let log_path = run_dir.map(|d| d.join("calls.jsonl"));
    let from_run = || -> Option<SimConfig> {
        let text = std::fs::read_to_string(run_dir?.join("config.json")).ok()?;
        serde_json::from_str(&text).ok()
    };
    let config = match config {
        Some(c) => c.clone(),
        None => from_run().unwrap_or_else(|| SimConfig {
            agent_runtime: settings.agent_runtime.clone(),
            planner_runtime: settings.planner_runtime.clone(),
            ..SimConfig::new("", Vec::new())
        }),
    };
    let concurrency = usize::try_from(concurrency).unwrap_or(1).max(1);
    build_llm(&config, &settings, concurrency, log_path, Some(think), LlmOptions::default())
        .map(|llm| Arc::new(llm) as Arc<dyn Llm>)
        .map_err(|e| LlmError::new(format!("{e}. Run `tellfold runtimes` to see what is available.")))
}

/// Explicit runtime ids, else the settings defaults; an unknown id is an error message naming the known ones.
pub fn resolve_runtimes(agent: Option<&str>, planner: Option<&str>) -> Result<(String, String), String> {
    let settings = load_settings(None);
    let known = known_ids(&settings);
    let pick = |given: Option<&str>, fallback: &str| given.filter(|v| !v.is_empty()).unwrap_or(fallback).to_string();
    let chosen = (pick(agent, &settings.agent_runtime), pick(planner, &settings.planner_runtime));
    for (flag, value) in [("--agent-runtime", &chosen.0), ("--planner-runtime", &chosen.1)] {
        if !known.contains(value) {
            return Err(format!(
                "unknown runtime {} for {flag}. Known: {}",
                crate::args::py_repr(value),
                known.join(", ")
            ));
        }
    }
    Ok(chosen)
}

#[cfg(test)]
mod tests {
    use super::delay_seconds;

    #[test]
    fn delays_parse_like_python_float() {
        for (raw, seconds) in [
            ("0", 0.0),
            (" 2.5 ", 2.5),
            ("1_0", 10.0),
            ("1_5e-1", 1.5),
            ("_5", 0.0),
            ("5_", 0.0),
            ("1__0", 0.0),
            ("1_.5", 0.0),
            ("99", 10.0),
            ("-3", 0.0),
            ("nan", 0.0),
            ("abc", 0.0),
        ] {
            assert_eq!(delay_seconds(raw), seconds, "{raw:?}");
        }
    }
}
