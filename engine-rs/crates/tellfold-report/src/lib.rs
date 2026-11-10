//! The report and everything built on it (port of `tellfold/report.py`, `chat.py` and `rewrite.py`): the evidence
//! digest, the report call with its citation audit and repair, Markdown and HTML rendering, interviews, report Q&A
//! and the rewrite suggestion.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tellfold_engine::runner::{PipelineError, ReportStage};
use tellfold_llm::{Llm, LlmError};
use tellfold_models::{KnowledgeGraph, Report, SimConfig};
use tellfold_store::files::write_json;
use tellfold_store::world::{World, WorldError};

pub mod audit;
pub mod chat;
pub mod evidence;
pub mod html;
pub mod pyre;
pub mod report;
pub mod rewrite;

#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error(transparent)]
    World(#[from] WorldError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Template(#[from] minijinja::Error),
    /// `RewriteError` in Python: the run has no editable seed, or it is too long to rewrite.
    #[error("{0}")]
    Rewrite(String),
    /// `KeyError` in Python: an interview with an agent the run does not have.
    #[error("{0}")]
    UnknownAgent(String),
}

impl From<ReportError> for PipelineError {
    fn from(e: ReportError) -> Self {
        match e {
            ReportError::Llm(e) => PipelineError::Llm(e),
            ReportError::World(e) => PipelineError::World(e),
            ReportError::Io(e) => PipelineError::Io(e),
            other => PipelineError::Other(other.to_string()),
        }
    }
}

/// `str.format` for the prompt templates: `{name}` placeholders filled in one pass, so a placeholder inside a filled
/// value stays as it is. The templates hold their literal braces single (`{"what", "why"}`), not doubled.
pub(crate) fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        let end = after.find('}').map_or(after.len(), |j| j + 1);
        let key = &after[1..end.saturating_sub(1).max(1)];
        match values.iter().find(|(k, _)| *k == key) {
            Some((_, v)) => out.push_str(v),
            None => out.push_str(&after[..end]),
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// `runner.write_report`: report.json, report.md and report.html; returns the HTML path.
pub fn write_report(
    run_dir: &Path,
    report: &Report,
    world: &World,
    config: &SimConfig,
) -> Result<PathBuf, ReportError> {
    write_json(&run_dir.join("report.json"), report)?;
    std::fs::write(run_dir.join("report.md"), report::render_markdown(report))?;
    let html_path = run_dir.join("report.html");
    std::fs::write(&html_path, html::render_html(report, world, config)?)?;
    Ok(html_path)
}

/// The pipeline's report stage: `build_report` with the planner model, then `write_report`.
pub struct ReportWriter;

#[async_trait(?Send)]
impl ReportStage for ReportWriter {
    async fn write_report(
        &self,
        llm: &dyn Llm,
        run_dir: &Path,
        world: &World,
        graph: &KnowledgeGraph,
        config: &SimConfig,
    ) -> Result<Report, PipelineError> {
        let built = report::build_report(llm, world, graph, config, &config.planner_model).await?;
        write_report(run_dir, &built, world, config)?;
        Ok(built)
    }
}
