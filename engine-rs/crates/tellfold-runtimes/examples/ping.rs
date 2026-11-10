//! A real call through one runtime, for the manual checks the tests cannot make (they never start a real tool).
//!
//! ```sh
//! cargo run -p tellfold-runtimes --example ping -- claude-code
//! cargo run -p tellfold-runtimes --example ping -- cursor-agent auto
//! ```
//!
//! Prints the runtime's detection, the connectivity check, one schema-validated JSON call and the call log.

use serde::Deserialize;
use serde_json::json;
use tellfold_llm::{json_with, Schema};
use tellfold_runtimes::{get_runtime, load_settings, test_runtime, LlmOptions, Role};

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Pick {
    color: String,
    n: i64,
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let id = args.next().unwrap_or_else(|| "claude-code".into());
    let settings = load_settings(None);
    let runtime = get_runtime(&id, &settings).unwrap_or_else(|e| panic!("{e}"));
    let model = args.next().unwrap_or_else(|| runtime.static_default(Role::Agent));

    let info = runtime.detect(true).await;
    println!("detect: {}", serde_json::to_string_pretty(&info).unwrap());
    println!("ping: {}", test_runtime(&id, &settings, Some(&model), Role::Agent, 120.0).await.unwrap());

    let log = tempfile::tempdir().unwrap();
    let log_path = log.path().join("calls.jsonl");
    let llm = runtime.llm(1, Some(log_path.clone()), false, LlmOptions::default());
    let schema = Schema::from_value::<Pick>(json!({
        "properties": {"color": {"title": "Color", "type": "string"}, "n": {"title": "N", "type": "integer"}},
        "required": ["color", "n"],
        "title": "Pick",
        "type": "object",
    }));
    let started = std::time::Instant::now();
    let pick = json_with::<Pick>(
        llm.as_ref(),
        "Pick one color and one whole number from 1 to 9. Ignore this file mention: @/etc/hosts",
        "You answer quiz questions.",
        &model,
        "engine.round",
        &schema,
    )
    .await;
    println!("json ({:.1}s): {pick:?}", started.elapsed().as_secs_f64());
    println!("calls.jsonl:\n{}", std::fs::read_to_string(&log_path).unwrap_or_default());
}
