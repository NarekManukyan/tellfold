//! Tellfold Studio's HTTP API (port of `tellfold/server`): the JSON API, the SSE stream and the single-page
//! frontend that `tellfold ui` serves and the Mac app talks to through `tellfold serve`.
//!
//! The app is meant to bind 127.0.0.1 only; [`routes::local_only`] additionally rejects foreign Host headers (DNS
//! rebinding) and cross-origin writes (a web page posting a form to localhost). With a token, every `/api` request
//! must carry `Authorization: Bearer <token>`.

pub mod error;
pub use tellfold_models::log;
pub mod manager;
pub mod routes;
pub mod upload;
pub mod validate;
pub mod web;

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Request, State};
use axum::middleware::{from_fn_with_state, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use futures::FutureExt;

use tellfold_runtimes::{get_runtime, load_settings};

pub use manager::{spawn_manager, Handle, LlmFactory, Manager, ManagerOptions, ManagerThread, RuntimeReady};

/// The version `/api/health` reports.
pub use tellfold_models::VERSION;
pub const DEFAULT_HOSTS: [&str; 4] = ["127.0.0.1", "localhost", "::1", "testserver"];

/// What every route can reach.
#[derive(Clone)]
pub struct AppState {
    pub handle: Handle,
    pub fake: bool,
    pub settings_path: Option<PathBuf>,
    /// seconds of silence before an SSE stream sends `: ping`
    pub ping_seconds: f64,
    allowed_hosts: Arc<HashSet<String>>,
    /// `Bearer <token>`, when `/api` needs one
    expected_auth: Option<Arc<Vec<u8>>>,
    bundle_hook: Option<BundleHook>,
}

/// Where a bundle request is when [`BundleHook`] is called.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleStage {
    /// on the manager thread, once an export was checked and marked, before the handler hears back
    Checked,
    /// on the blocking worker, before a bundle is written or opened
    Work,
}

/// Called at each [`BundleStage`] of a bundle request. Tests hold the request there: on the worker to show the server
/// keeps answering meanwhile, on the manager to drop a request whose reply is not sent yet.
#[derive(Clone)]
pub struct BundleHook(pub Arc<dyn Fn(BundleStage) + Send + Sync>);

impl std::fmt::Debug for BundleHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BundleHook")
    }
}

/// `create_app`'s keyword arguments.
#[derive(Clone, Debug)]
pub struct AppOptions {
    pub fake_llm: bool,
    pub token: Option<String>,
    pub allowed_hosts: Option<Vec<String>>,
    pub ping_seconds: f64,
    #[doc(hidden)]
    pub bundle_hook: Option<BundleHook>,
}

impl Default for AppOptions {
    fn default() -> Self {
        AppOptions {
            fake_llm: false,
            token: None,
            allowed_hosts: None,
            ping_seconds: routes::PING_SECONDS,
            bundle_hook: None,
        }
    }
}

/// Whether the AI tools the settings name for agents and planner are installed (no model call).
pub fn runtime_installed(settings_path: Option<PathBuf>) -> RuntimeReady {
    Arc::new(move || {
        let path = settings_path.clone();
        async move {
            let settings = load_settings(path.as_deref());
            let ids: BTreeSet<String> = [settings.agent_runtime.clone(), settings.planner_runtime.clone()].into();
            for id in ids {
                match get_runtime(&id, &settings) {
                    Ok(runtime) if runtime.detect(false).await.installed => {}
                    _ => return false,
                }
            }
            true
        }
        .boxed()
    })
}

async fn local_only_guard(State(app): State<AppState>, request: Request, next: Next) -> Response {
    if let Some(refused) = routes::local_only(&app.allowed_hosts, request.method(), request.headers()) {
        return refused;
    }
    next.run(request).await
}

async fn token_guard(State(app): State<AppState>, request: Request, next: Next) -> Response {
    if let Some(expected) = &app.expected_auth {
        let path = routes::percent_decode(request.uri().path());
        if (path == "/api" || path.starts_with("/api/")) && !routes::token_ok(expected, request.headers()) {
            return routes::unauthorized();
        }
    }
    next.run(request).await
}

/// The Tellfold Studio app and its run manager. Without `options.fake_llm` and a `runtime_ready` check, the
/// settings' tools must be installed to interview people in an example.
pub fn create_app(mut manager: ManagerOptions, options: AppOptions) -> (Router, ManagerThread) {
    if manager.runtime_ready.is_none() && !options.fake_llm {
        manager.runtime_ready = Some(runtime_installed(manager.settings_path.clone()));
    }
    // TokenAuthMiddleware's ValueError: an empty token would let a bare "Bearer " through
    assert!(options.token.as_deref() != Some(""), "an empty token would disable authentication");
    let _ = std::fs::create_dir_all(&manager.runs_dir);
    tellfold_study::bundle::sweep_staging(&manager.runs_dir);
    let settings_path = manager.settings_path.clone();
    let thread = spawn_manager(manager);
    let hosts = options.allowed_hosts.unwrap_or_else(|| DEFAULT_HOSTS.iter().map(|h| h.to_string()).collect());
    let state = AppState {
        handle: thread.handle.clone(),
        fake: options.fake_llm,
        settings_path,
        ping_seconds: options.ping_seconds,
        allowed_hosts: Arc::new(hosts.iter().map(|h| h.to_lowercase().trim_matches(['[', ']']).to_string()).collect()),
        expected_auth: options.token.map(|t| Arc::new(format!("Bearer {t}").into_bytes())),
        bundle_hook: options.bundle_hook,
    };
    (router(state), thread)
}

pub fn router(state: AppState) -> Router {
    use routes::*;
    Router::new()
        .route("/api/health", get(health))
        .route(
            "/api/runs",
            get(list_runs).post(post_runs).layer(DefaultBodyLimit::max(upload::MAX_FORM_BYTES + 1024 * 1024)),
        )
        .route(
            "/api/runs/import",
            // the handler streams the body and holds it to MAX_BUNDLE_BYTES itself
            post(import_bundle),
        )
        .route("/api/runs/{run_id}/bundle", get(export_bundle))
        .route("/api/studies", get(list_studies))
        .route("/api/studies/{study_id}", get(get_study))
        .route("/api/runs/{run_id}/branch", post(branch))
        .route("/api/runs/{run_id}/rewrite", post(rewrite).get(saved_rewrite))
        .route("/api/runs/{run_id}/check", post(check_run))
        .route("/api/studies/{study_id}/compare", post(compare_study))
        .route("/api/runs/{run_id}", get(get_run))
        .route("/api/runs/{run_id}/posts", get(get_posts))
        .route("/api/runs/{run_id}/actions", get(get_actions))
        .route("/api/runs/{run_id}/agents/{agent_id}", get(get_agent))
        .route("/api/runs/{run_id}/events", post(inject_event))
        .route("/api/runs/{run_id}/stop", post(stop_run))
        .route("/api/runs/{run_id}/personas", get(get_cast).put(put_cast))
        .route("/api/runs/{run_id}/personas/confirm", post(confirm_cast))
        .route("/api/runs/{run_id}/personas/group", post(add_group))
        .route("/api/runs/{run_id}/chat", post(chat))
        .route("/api/runs/{run_id}/ask", post(ask))
        .route("/api/runs/{run_id}/report", post(rebuild_report))
        .route("/api/runs/{run_id}/seeds", get(list_seeds))
        .route("/api/runs/{run_id}/seeds/{name}", get(get_seed))
        .route("/api/runs/{run_id}/report.html", get(report_html))
        .route("/api/runs/{run_id}/report.md", get(report_markdown))
        .route("/api/runs/{run_id}/stream", get(stream))
        .route("/api/runtimes", get(list_runtimes))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/runtimes/{runtime_id}/test", post(test_runtime_route))
        .route("/static/{*path}", get(static_file))
        .fallback(fallback)
        .method_not_allowed_fallback(fallback)
        .layer(from_fn_with_state(state.clone(), local_only_guard))
        .layer(from_fn_with_state(state.clone(), token_guard))
        .with_state(state)
}
