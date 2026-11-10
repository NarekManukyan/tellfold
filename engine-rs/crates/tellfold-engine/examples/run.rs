//! A real run through a real runtime, for the manual checks the tests cannot make (they never start a real tool).
//! The report stage is skipped here; `cargo run -p tellfold-report --example run` runs the same with the real report.
//!
//! ```sh
//! cargo run -p tellfold-engine --example run -- ../examples/mossbook_pricing.md "Will buyers accept the subscription?"
//! ```
//!
//! Optional environment: `AGENTS` (default 4), `ROUNDS` (default 1), `RUNTIME` (default claude-code).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};
use tellfold_engine::runner::{run_pipeline, PipelineError, ReportStage};
use tellfold_llm::Llm;
use tellfold_models::{KnowledgeGraph, Report, SimConfig};
use tellfold_runtimes::{build_llm, load_settings, LlmOptions};
use tellfold_store::world::World;

struct SkipReport;

#[async_trait(?Send)]
impl ReportStage for SkipReport {
    async fn write_report(
        &self,
        _: &dyn Llm,
        _: &Path,
        _: &World,
        _: &KnowledgeGraph,
        _: &SimConfig,
    ) -> Result<Report, PipelineError> {
        Ok(serde_json::from_value(json!({"title": "-", "summary": "-", "prediction": "-", "confidence": "low"}))
            .expect("valid report"))
    }
}

fn env_int(name: &str, default: i64) -> i64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let seed = PathBuf::from(args.next().expect("seed file"));
    let question = args.next().unwrap_or_else(|| "How will people react?".into());
    let runtime = std::env::var("RUNTIME").unwrap_or_else(|_| "claude-code".into());
    let config = SimConfig {
        num_agents: env_int("AGENTS", 4),
        rounds: env_int("ROUNDS", 1),
        batch_size: 4,
        agent_runtime: runtime.clone(),
        planner_runtime: runtime,
        ..SimConfig::new(&question, vec![seed.to_string_lossy().into()])
    };
    let run_dir = tempfile::Builder::new().prefix("tellfold-example-run-").tempdir().expect("temp dir").keep();
    let settings = load_settings(None);
    let llm = build_llm(&config, &settings, 4, Some(run_dir.join("calls.jsonl")), None, LlmOptions::default())
        .unwrap_or_else(|e| panic!("{e}"));
    let on_event = |name: &str, payload: &Value| {
        if name != "status" {
            println!("{name}: {payload}");
        }
    };
    let started = std::time::Instant::now();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    let result =
        runtime.block_on(run_pipeline(&llm, &run_dir, &config, &[seed], Some(&on_event), None, None, &SkipReport));
    println!("result after {:.0}s: {:?}", started.elapsed().as_secs_f64(), result.map(|r| r.is_some()));
    println!("run folder: {}", run_dir.display());
}
