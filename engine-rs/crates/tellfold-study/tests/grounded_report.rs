//! The report of a run built from audience material names only what the planner actually read (Rust-only).

use std::future::Future;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tellfold_engine::material::{read_material, write_material, MaterialFile};
use tellfold_engine::runner::{run_pipeline, RunOptions};
use tellfold_llm::canned::fake_responder;
use tellfold_llm::fake::{FakeLlm, FakeReply};
use tellfold_llm::{LlmError, Schema};
use tellfold_models::SimConfig;
use tellfold_report::ReportWriter;
use tellfold_store::files::copy_seeds;

fn block_on<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}

/// The report's caveats after a grounded run of three reviews with `llm`.
fn caveats(llm: &FakeLlm) -> String {
    let tmp = tempfile::tempdir().unwrap();
    let run_dir = tmp.path().join("20261110-090604-grounded");
    std::fs::create_dir_all(&run_dir).unwrap();
    let seed: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../examples/mossbook_pricing.md");
    let seeds = copy_seeds(&run_dir, &[seed]).unwrap();
    let reviews =
        "The price doubled overnight, not happy.\n\nI love the offline mode a lot.\n\nSync keeps failing on my phone.";
    let file = MaterialFile { name: "r.txt".into(), kind: Some("reviews".into()), column: None, bytes: reviews.into() };
    let material = read_material(&[file], &[], None).unwrap();
    write_material(&run_dir, &material).unwrap();
    let mut config = SimConfig::new(
        "Will buyers accept the subscription?",
        seeds.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
    );
    (config.num_agents, config.rounds, config.batch_size) = (6, 2, 3);
    config.material = Some(material.summary);
    block_on(run_pipeline(llm, &run_dir, &config, &seeds, &ReportWriter, RunOptions::default())).unwrap().unwrap();
    let report: Value = serde_json::from_str(&std::fs::read_to_string(run_dir.join("report.json")).unwrap()).unwrap();
    report["caveats"].to_string()
}

#[test]
fn material_nobody_could_read_gets_no_caveat() {
    let llm = FakeLlm::new(|purpose: &str, prompt: &str, schema: Option<&Schema>| -> FakeReply {
        if purpose == "material.read" {
            return LlmError("simulated failure".into()).into();
        }
        fake_responder(purpose, prompt, schema)
    });
    let caveats = caveats(&llm);
    assert!(!caveats.contains("based on"), "{caveats}");
}

#[test]
fn material_read_in_part_names_what_was_read() {
    let llm = FakeLlm::new(|purpose: &str, prompt: &str, schema: Option<&Schema>| -> FakeReply {
        if purpose == "material.read" {
            return json!({"items": [{"item_id": "m001", "camp": "lean_critical", "theme": "price doubled"}]}).into();
        }
        fake_responder(purpose, prompt, schema)
    });
    let caveats = caveats(&llm);
    assert!(caveats.contains("The public is based on 1 review you supplied"), "{caveats}");
}
