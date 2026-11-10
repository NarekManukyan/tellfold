//! Agent runtimes: detect the LLM tools installed on this machine and drive any of them through [`Llm`] (port of
//! `tellfold/settings.py` and `tellfold/runtimes/`).
//!
//! [`build_llm`] sends `engine.*` and `chat.interview` calls to the agent runtime and everything else (graph,
//! personas, report, ask) to the planner runtime.

pub mod claude;
pub mod cli;
pub mod detect;
pub mod http;
pub mod process;
pub mod registry;
pub mod settings;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::{json, Value};
use tellfold_llm::adapter::{CLAUDE_ALIASES, TOOL_DEFAULT};
use tellfold_llm::{Llm, LlmError, Schema};
use tellfold_models::json::py_round;
use tellfold_models::text::{py_prefix, py_strip};
use tellfold_models::SimConfig;

pub use registry::{
    all_runtimes, get_runtime, known_ids, Auth, Kind, LlmOptions, Role, Runtime, RuntimeInfo, UnknownRuntime,
};
pub use settings::{load_settings, save_settings, OpenAiEndpoint, Settings};
pub use tellfold_models::{is_agent_purpose, AGENT_PURPOSES};

/// Every runtime with its status, probed concurrently (version calls time out after 3 s each).
pub async fn detect_all(settings: &Settings, check_auth: bool) -> Vec<RuntimeInfo> {
    let runtimes = all_runtimes(settings);
    futures::future::join_all(runtimes.iter().map(|r| r.detect(check_auth))).await
}

/// One side of a [`RoutedLlm`].
#[derive(Clone)]
pub struct Route {
    pub llm: Arc<dyn Llm>,
    pub runtime_id: String,
    pub default_model: String,
    pub accepts_claude_aliases: bool,
}

impl Route {
    /// The caller's model, unless it is a Claude alias (or empty) sent to a runtime that cannot use one.
    pub fn model<'a>(&'a self, requested: &'a str) -> &'a str {
        if self.accepts_claude_aliases {
            return requested;
        }
        if requested.is_empty() || CLAUDE_ALIASES.contains(&requested) {
            return if self.default_model.is_empty() { TOOL_DEFAULT } else { &self.default_model };
        }
        requested
    }
}

/// `LLM` that sends agent purposes to one backend and planner purposes to another (or the same) one.
pub struct RoutedLlm {
    pub agent: Route,
    pub planner: Route,
}

impl RoutedLlm {
    pub fn route(&self, purpose: &str) -> &Route {
        if is_agent_purpose(purpose) {
            &self.agent
        } else {
            &self.planner
        }
    }
}

#[async_trait]
impl Llm for RoutedLlm {
    async fn text(&self, prompt: &str, system: &str, model: &str, purpose: &str) -> Result<String, LlmError> {
        let route = self.route(purpose);
        route.llm.text(prompt, system, route.model(model), purpose).await
    }

    async fn json_value(
        &self,
        prompt: &str,
        system: &str,
        model: &str,
        purpose: &str,
        schema: &Schema,
    ) -> Result<Value, LlmError> {
        let route = self.route(purpose);
        route.llm.json_value(prompt, system, route.model(model), purpose, schema).await
    }
}

fn route(runtime: &Runtime, llm: Arc<dyn Llm>, role: Role) -> Route {
    Route {
        llm,
        runtime_id: runtime.id(),
        default_model: runtime.static_default(role),
        accepts_claude_aliases: runtime.accepts_claude_aliases(),
    }
}

/// One LLM for a run: agent purposes go to `config.agent_runtime`, the rest to `config.planner_runtime`.
///
/// Fails with [`UnknownRuntime`] for an id that is not in the catalogue. `think` defaults to `config.think`;
/// `options` (timeout, retries) are passed to the adapters.
pub fn build_llm(
    config: &SimConfig,
    settings: &Settings,
    concurrency: usize,
    log_path: Option<PathBuf>,
    think: Option<bool>,
    options: LlmOptions,
) -> Result<RoutedLlm, UnknownRuntime> {
    let think = think.unwrap_or(config.think);
    let agent_rt = get_runtime(&config.agent_runtime, settings)?;
    let planner_rt = if config.planner_runtime == config.agent_runtime {
        None
    } else {
        Some(get_runtime(&config.planner_runtime, settings)?)
    };
    let agent_llm = agent_rt.llm(concurrency, log_path.clone(), think, options);
    let planner_llm = match &planner_rt {
        None => agent_llm.clone(),
        Some(rt) => rt.llm(concurrency, log_path, think, options),
    };
    let agent = route(&agent_rt, agent_llm, Role::Agent);
    let planner = route(planner_rt.as_ref().unwrap_or(&agent_rt), planner_llm, Role::Planner);
    Ok(RoutedLlm { agent, planner })
}

pub const PING_PROMPT: &str = "Reply with the single word: ok";
pub const PING_SYSTEM: &str = "You are a connectivity check. Answer with one word.";

/// One tiny real call: `{ok, seconds, reply, error, model}`. Fails with [`UnknownRuntime`] for an unknown id.
pub async fn test_runtime(
    runtime_id: &str,
    settings: &Settings,
    model: Option<&str>,
    role: Role,
    timeout_s: f64,
) -> Result<Value, UnknownRuntime> {
    let runtime = get_runtime(runtime_id, settings)?;
    let chosen = match model.filter(|m| !m.is_empty()) {
        Some(m) => m.to_string(),
        None => match runtime.static_default(role) {
            d if d.is_empty() => TOOL_DEFAULT.to_string(),
            d => d,
        },
    };
    let llm = runtime.llm(1, None, false, LlmOptions { timeout_s: Some(timeout_s), retries: Some(0) });
    let started = Instant::now();
    let reply = llm.text(PING_PROMPT, PING_SYSTEM, &chosen, "runtime.test").await;
    let seconds = py_round(started.elapsed().as_secs_f64(), 2);
    Ok(match reply {
        Ok(reply) => {
            let reply = py_strip(&reply);
            json!({"ok": !reply.is_empty(), "seconds": seconds, "reply": py_prefix(reply, 200), "error": null,
                   "model": chosen})
        }
        Err(e) => {
            json!({"ok": false, "seconds": seconds, "reply": null, "error": py_prefix(&e.0, 500), "model": chosen})
        }
    })
}
