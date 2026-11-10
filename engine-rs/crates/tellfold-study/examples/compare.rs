//! A real comparison and Check it of two finished runs through a real runtime, for the manual checks the tests
//! cannot make (they never start a real tool). Prints both results as JSON.
//!
//! ```sh
//! cargo run -p tellfold-study --example compare -- RUN_A RUN_B
//! ```
//!
//! Optional environment: `RUNTIME` (default claude-code).

use std::path::PathBuf;

use tellfold_models::SimConfig;
use tellfold_runtimes::{build_llm, load_settings, LlmOptions};
use tellfold_study::agree::build_agreement;
use tellfold_study::compare::{build_compare, Member};

fn main() {
    let mut args = std::env::args().skip(1);
    let a = PathBuf::from(args.next().expect("first run folder"));
    let b = PathBuf::from(args.next().expect("second run folder"));
    let runtime = std::env::var("RUNTIME").unwrap_or_else(|_| "claude-code".into());
    let text = std::fs::read_to_string(a.join("config.json")).expect("config.json");
    let mut config: SimConfig = serde_json::from_str(&text).expect("a run config");
    config.agent_runtime = runtime.clone();
    config.planner_runtime = runtime;
    let llm = build_llm(&config, &load_settings(None), 2, None, None, LlmOptions::default())
        .unwrap_or_else(|e| panic!("{e}"));
    let members = [Member::new("A", "a", "Draft A", &a), Member::new("B", "b", "Draft B", &b)];
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    let started = std::time::Instant::now();
    let compare =
        rt.block_on(build_compare(&llm, "live", &members, &config.question, &config.planner_model, "English", None));
    println!("compare after {:.0}s:", started.elapsed().as_secs_f64());
    println!("{}", serde_json::to_string_pretty(&compare.expect("compare")).unwrap());
    let started = std::time::Instant::now();
    let agreement =
        rt.block_on(build_agreement(&llm, "live", &members, &config.question, &config.planner_model, "English"));
    println!("agreement after {:.0}s:", started.elapsed().as_secs_f64());
    println!("{}", serde_json::to_string_pretty(&agreement.expect("agreement")).unwrap());
}
