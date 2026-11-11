//! The JSON shapes of the HTTP API that are built from a run folder (`tellfold/server/app.py`).

use serde::Serialize;
use tellfold_models::{GodEvent, KnowledgeGraph, Persona, Post, Report, RoundStats, RunStatus, SimConfig};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunSummary {
    pub id: String,
    pub question: String,
    pub state: String,
    pub stage: String,
    pub round: i64,
    pub total_rounds: i64,
    pub agents: i64,
    pub created_at: String,
    pub has_report: bool,
    /// A built-in example run: read-only.
    pub example: bool,
    /// A run imported from a `.tellfold` bundle: read-only. Left out when false.
    #[serde(skip_serializing_if = "is_false")]
    pub imported: bool,
    pub study_id: Option<String>,
    pub variant: Option<String>,
    /// `RunStatus.pause`: why a running run waits for a client. Rust-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pause: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CallTotals {
    pub count: i64,
    pub failed: i64,
    pub seconds: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CallsSummary {
    pub count: i64,
    pub failed: i64,
    pub seconds: f64,
    /// engine.* and chat.interview calls
    pub agent: CallTotals,
    /// graph, personas, report and ask calls
    pub planner: CallTotals,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ReportRebuild {
    pub running: bool,
    /// Why the last rebuild started in this server failed.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunDetail {
    pub id: String,
    pub config: SimConfig,
    pub status: Option<RunStatus>,
    pub live: bool,
    pub personas: Vec<Persona>,
    pub graph: Option<KnowledgeGraph>,
    pub stats: Vec<RoundStats>,
    pub events: Vec<GodEvent>,
    pub follows: Vec<[String; 2]>,
    pub report: Option<Report>,
    pub calls: CallsSummary,
    pub report_rebuild: ReportRebuild,
    /// A built-in example run: events, stop and report rebuild are refused.
    pub example: bool,
    /// A run imported from a `.tellfold` bundle, refused the same changes. Left out when false.
    #[serde(skip_serializing_if = "is_false")]
    pub imported: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PostOut {
    #[serde(flatten)]
    pub post: Post,
    pub author_handle: String,
    pub sentiment: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActionOut {
    pub round: i64,
    pub agent_id: String,
    pub r#type: String,
    pub target_post_id: Option<i64>,
    pub target_agent_id: Option<String>,
    pub content: String,
    pub reasoning: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentDetail {
    pub persona: Persona,
    pub memory: Vec<String>,
    pub posts: Vec<PostOut>,
    pub followers: Vec<String>,
    pub following: Vec<String>,
    pub sentiment_by_round: Vec<(i64, f64)>,
}
