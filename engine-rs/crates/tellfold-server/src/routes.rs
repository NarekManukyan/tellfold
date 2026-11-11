//! The JSON API, the SSE stream and the frontend routes of `tellfold/server/app.py` and `runtime_routes.py`.
//!
//! Handlers run their work on the manager thread through [`Handle::call`], so reads and writes of a run happen in
//! the same order as on the Python server's single event loop. Bodies and query parameters are validated before a
//! handler looks at the run, as FastAPI does. Making and opening `.tellfold` bundles is the exception: the manager only
//! checks the run, and the zip work runs on a blocking worker so live runs and other requests are not held up.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method};
use axum::response::{IntoResponse, Response};
use regex::Regex;
use serde_json::{json, Value};

use tellfold_engine::personas::MAX_CAST;
use tellfold_models::text::py_strip;
use tellfold_models::{GodEvent, Study, StudyKind};
use tellfold_runtimes::{detect_all, known_ids, load_settings, save_settings, test_runtime, Role, Settings};
use tellfold_store::api::RunSummary;
use tellfold_study::compare::digest_run;

use crate::error::{ok_json, ApiError, ApiResult};
use crate::manager::{history_pairs, Closed, Handle, Manager, ManagerError, Queue, ReadOnly};
use crate::upload::create_run;
use crate::validate::{parse_body, query_int, Checker};
use crate::{web, AppState, VERSION};

pub const PING_SECONDS: f64 = 15.0;

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid pattern"))
}

/// [`tellfold_store::files::is_run_id`] as a pattern, for `py_match`.
pub fn run_id_pattern() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(&format!(r"^[A-Za-z0-9][A-Za-z0-9._-]{{0,{}}}$", tellfold_store::files::MAX_RUN_ID_LEN - 1))
            .expect("valid pattern")
    })
}

pub fn agent_id_pattern() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    regex(&R, r"^@?[A-Za-z0-9_.-]{1,64}$")
}

/// "/" for provider/model names
pub fn model_pattern() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    regex(&R, r"^[A-Za-z0-9][A-Za-z0-9._:/@\[\]-]{0,119}$")
}

pub fn runtime_id_pattern() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    regex(&R, r"^[a-z0-9][a-z0-9._-]{0,39}(:[a-z0-9][a-z0-9_-]{0,39})?$")
}

pub fn study_id_pattern() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    regex(&R, r"^[A-Za-z0-9][A-Za-z0-9._-]{0,99}$")
}

/// `pattern.match(text)` for a pattern ending in `$`, which in Python also matches before a final newline.
pub fn py_match(pattern: &Regex, text: &str) -> bool {
    pattern.is_match(text) || text.strip_suffix('\n').is_some_and(|t| pattern.is_match(t))
}

impl From<Closed> for ApiError {
    fn from(closed: Closed) -> Self {
        match closed {
            Closed::ShuttingDown => ApiError::new(503, "the server is shutting down"),
            // FastAPI's answer to an unhandled exception
            Closed::Panicked => ApiError::internal("a manager job panicked"),
        }
    }
}

impl From<ManagerError> for ApiError {
    fn from(e: ManagerError) -> Self {
        match e {
            ManagerError::Conflict(m) => ApiError::new(409, m),
            ManagerError::BadRequest(m) => ApiError::new(400, m),
            ManagerError::ReadOnly(m) => ApiError::new(403, m),
            ManagerError::UnknownAgent => ApiError::new(404, "agent not found"),
            ManagerError::Llm(m) => ApiError::new(502, format!("the model call failed: {m}")),
            ManagerError::Internal(m) => ApiError::internal(m),
        }
    }
}

/// Runs `f` on the manager thread; its error is the response.
async fn on_manager<F, Fut>(handle: &Handle, f: F) -> ApiResult
where
    F: FnOnce(Rc<Manager>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ApiResult> + 'static,
{
    handle.call(f).await?
}

/// The folder of a run: 400 for a malformed id, 404 unless it is a run (or an example).
pub fn run_dir_of(m: &Manager, run_id: &str) -> Result<PathBuf, ApiError> {
    if !py_match(run_id_pattern(), run_id) || run_id.contains("..") {
        return Err(ApiError::new(400, "invalid run id"));
    }
    if m.is_example(run_id) {
        return Ok(m.run_path(run_id));
    }
    let path = m.runs_dir.join(run_id);
    let resolve = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if resolve(&path).parent() != Some(resolve(&m.runs_dir).as_path()) || !path.join("config.json").is_file() {
        return Err(ApiError::new(404, "run not found"));
    }
    Ok(path)
}

/// The last value of each query parameter (Starlette's `QueryParams.get`).
fn last(query: &[(String, String)], key: &str) -> Option<String> {
    query.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

// ---------- runs ----------

pub async fn health(State(app): State<AppState>) -> Response {
    ok_json(&json!({"ok": true, "fake_llm": app.fake, "version": VERSION}))
}

pub async fn list_runs(State(app): State<AppState>) -> ApiResult {
    on_manager(&app.handle, |m| async move {
        let live = m.live_ids();
        let examples = m.examples();
        let mut out: Vec<RunSummary> = Vec::new();
        for id in examples.keys() {
            if let Some(mut summary) = m.reader(id).summary(false) {
                summary.example = true;
                out.push(summary);
            }
        }
        if let Ok(entries) = std::fs::read_dir(&m.runs_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().into_owned();
                // a normal run can never take an example's id: the example would shadow it on every route
                if !entry.path().is_dir() || !py_match(run_id_pattern(), &name) || examples.contains_key(&name) {
                    continue;
                }
                if let Some(mut summary) = m.reader(&name).summary(live.contains(&name)) {
                    summary.imported = m.is_imported(&name);
                    out.push(summary);
                }
            }
        }
        out.sort_by(|a, b| (&b.created_at, &b.id).cmp(&(&a.created_at, &a.id)));
        Ok(ok_json(&out))
    })
    .await
}

pub async fn post_runs(State(app): State<AppState>, request: Request) -> ApiResult {
    create_run(app, request).await
}

pub async fn get_run(State(app): State<AppState>, UrlPath(run_id): UrlPath<String>) -> ApiResult {
    on_manager(&app.handle, move |m| async move {
        run_dir_of(&m, &run_id)?;
        let live = m.is_live(&run_id);
        let Some(mut detail) = m.reader(&run_id).detail(live) else { return Err(ApiError::new(404, "run not found")) };
        if detail.status.is_none() {
            detail.status = m.effective_status(&run_id);
        }
        detail.report_rebuild = m.rebuild_state(&run_id);
        let read_only = m.read_only(&run_id);
        detail.example = read_only == Some(ReadOnly::Example);
        detail.imported = read_only == Some(ReadOnly::Imported);
        Ok(ok_json(&detail))
    })
    .await
}

pub async fn get_posts(
    State(app): State<AppState>,
    UrlPath(run_id): UrlPath<String>,
    Query(query): Query<Vec<(String, String)>>,
) -> ApiResult {
    let since = query_int(last(&query, "since_id").as_deref(), "since_id", Some(0), Some(0), None)?.unwrap_or(0);
    on_manager(&app.handle, move |m| async move {
        run_dir_of(&m, &run_id)?;
        Ok(ok_json(&m.reader(&run_id).posts(since)))
    })
    .await
}

pub async fn get_actions(
    State(app): State<AppState>,
    UrlPath(run_id): UrlPath<String>,
    Query(query): Query<Vec<(String, String)>>,
) -> ApiResult {
    let round = query_int(last(&query, "round").as_deref(), "round", None, Some(0), None)?;
    on_manager(&app.handle, move |m| async move {
        run_dir_of(&m, &run_id)?;
        Ok(ok_json(&m.reader(&run_id).actions(round)))
    })
    .await
}

pub async fn get_agent(
    State(app): State<AppState>,
    UrlPath((run_id, agent_id)): UrlPath<(String, String)>,
) -> ApiResult {
    on_manager(&app.handle, move |m| async move {
        run_dir_of(&m, &run_id)?;
        if !py_match(agent_id_pattern(), &agent_id) {
            return Err(ApiError::new(400, "invalid agent id"));
        }
        let reader = m.reader(&run_id);
        let agent = reader.agent(&agent_id).or_else(|| {
            // accept a handle too
            let wanted = agent_id.trim_start_matches('@').to_lowercase();
            let id = reader.personas().into_iter().find(|p| p.handle.to_lowercase() == wanted).map(|p| p.id)?;
            reader.agent(&id)
        });
        match agent {
            Some(agent) => Ok(ok_json(&agent)),
            None => Err(ApiError::new(404, "agent not found")),
        }
    })
    .await
}

/// `EventIn`: `{round: int, content: str (1-2000)}`.
fn event_in(checker: &mut Checker, value: &Value, loc: &[Value], class: Option<&str>) -> Option<GodEvent> {
    let obj = checker.object(value, loc, class)?;
    let round = checker.int(obj, "round", loc, None, None, None);
    let content = checker.string(obj, "content", loc, true, Some(1), Some(2000), None);
    Some(GodEvent::new(round?, &content?))
}

pub async fn inject_event(State(app): State<AppState>, UrlPath(run_id): UrlPath<String>, body: Bytes) -> ApiResult {
    let value = parse_body(&body, true)?.unwrap_or(Value::Null);
    let mut checker = Checker::new();
    let event = event_in(&mut checker, &value, &[json!("body")], None);
    checker.finish()?;
    let event = event.expect("validated");
    on_manager(&app.handle, move |m| async move {
        run_dir_of(&m, &run_id)?;
        m.inject_event(&run_id, event.round, &event.content)?;
        Ok(ok_json(&json!({"ok": true})))
    })
    .await
}

pub async fn stop_run(State(app): State<AppState>, UrlPath(run_id): UrlPath<String>) -> ApiResult {
    on_manager(&app.handle, move |m| async move {
        run_dir_of(&m, &run_id)?;
        m.stop(&run_id)?;
        Ok(ok_json(&json!({"ok": true})))
    })
    .await
}

// ---------- the cast of a run waiting at personas_ready ----------

/// `{waiting, personas}`: the cast under review, or the run's people once it is past that point.
pub async fn get_cast(State(app): State<AppState>, UrlPath(run_id): UrlPath<String>) -> ApiResult {
    on_manager(&app.handle, move |m| async move {
        run_dir_of(&m, &run_id)?;
        Ok(ok_json(&m.cast(&run_id)))
    })
    .await
}

/// Replaces the cast of a waiting run with `{personas: [Persona] (0-200)}`; returns `{waiting, personas}` as tidied.
/// An empty cast is kept while the run waits, so a group can still be added; the confirm needs at least one person.
pub async fn put_cast(State(app): State<AppState>, UrlPath(run_id): UrlPath<String>, body: Bytes) -> ApiResult {
    let value = parse_body(&body, true)?.unwrap_or(Value::Null);
    let mut checker = Checker::new();
    let loc = [json!("body")];
    let personas = checker.object(&value, &loc, None).and_then(|obj| {
        if !obj.contains_key("personas") {
            checker.error("missing", vec![loc[0].clone(), json!("personas")], "Field required".into(), &value, None);
            return None;
        }
        let items = checker.list(obj, "personas", &loc, Some(MAX_CAST))?;
        let mut personas = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            match serde_json::from_value::<tellfold_models::Persona>(item.clone()) {
                Ok(p) => personas.push(p),
                Err(e) => {
                    let at = vec![loc[0].clone(), json!("personas"), json!(i)];
                    checker.error("model_type", at, format!("Input should be a valid Persona: {e}"), item, None);
                }
            }
        }
        Some(personas)
    });
    checker.finish()?;
    let Some(personas) = personas else { unreachable!("validated") };
    on_manager(&app.handle, move |m| async move {
        run_dir_of(&m, &run_id)?;
        let personas = m.replace_cast(&run_id, personas)?;
        Ok(ok_json(&json!({"waiting": true, "personas": personas})))
    })
    .await
}

/// Lets a waiting run go on with its cast.
pub async fn confirm_cast(State(app): State<AppState>, UrlPath(run_id): UrlPath<String>) -> ApiResult {
    on_manager(&app.handle, move |m| async move {
        run_dir_of(&m, &run_id)?;
        m.confirm_cast(&run_id)?;
        Ok(ok_json(&json!({"ok": true})))
    })
    .await
}

/// `{description: str (1-500), count: int (1-20, default 3)}`: the engine writes that group and adds it to the
/// waiting cast; returns `{waiting, personas}`.
pub async fn add_group(State(app): State<AppState>, UrlPath(run_id): UrlPath<String>, body: Bytes) -> ApiResult {
    let value = parse_body(&body, true)?.unwrap_or(Value::Null);
    let mut checker = Checker::new();
    let loc = [json!("body")];
    let fields = checker.object(&value, &loc, None).map(|obj| {
        (
            checker.string(obj, "description", &loc, true, Some(1), Some(500), None),
            checker.int(obj, "count", &loc, Some(3), Some(1), Some(tellfold_engine::personas::MAX_GROUP as i64)),
        )
    });
    if let Some((Some(description), _)) = &fields {
        // only whitespace is as empty as nothing; generate_group trims what it uses
        if py_strip(description).is_empty() {
            let at = vec![loc[0].clone(), json!("description")];
            let input = json!(description);
            checker.error(
                "string_too_short",
                at,
                "String should have at least 1 character".into(),
                &input,
                Some(json!({"min_length": 1})),
            );
        }
    }
    checker.finish()?;
    let Some((Some(description), Some(count))) = fields else { unreachable!("validated") };
    on_manager(&app.handle, move |m| async move {
        run_dir_of(&m, &run_id)?;
        let personas = m.add_group(&run_id, &description, count as usize).await?;
        Ok(ok_json(&json!({"waiting": true, "personas": personas})))
    })
    .await
}

pub async fn chat(State(app): State<AppState>, UrlPath(run_id): UrlPath<String>, body: Bytes) -> ApiResult {
    let value = parse_body(&body, true)?.unwrap_or(Value::Null);
    let mut checker = Checker::new();
    let loc = [json!("body")];
    let fields = checker.object(&value, &loc, None).map(|obj| {
        (
            checker.string(obj, "agent_id", &loc, true, Some(1), Some(64), None),
            checker.string(obj, "message", &loc, true, Some(1), Some(4000), None),
            checker.list(obj, "history", &loc, Some(50)).cloned(),
        )
    });
    checker.finish()?;
    let Some((Some(agent_id), Some(message), Some(history))) = fields else { unreachable!("validated") };
    on_manager(&app.handle, move |m| async move {
        run_dir_of(&m, &run_id)?;
        if !py_match(agent_id_pattern(), &agent_id) {
            return Err(ApiError::new(400, "invalid agent id"));
        }
        let history = history_pairs(&history)?;
        let history = &history[history.len().saturating_sub(20)..];
        let reply = m.chat(&run_id, &agent_id, py_strip(&message), history).await?;
        Ok(ok_json(&json!({"reply": reply})))
    })
    .await
}

pub async fn ask(State(app): State<AppState>, UrlPath(run_id): UrlPath<String>, body: Bytes) -> ApiResult {
    let value = parse_body(&body, true)?.unwrap_or(Value::Null);
    let mut checker = Checker::new();
    let loc = [json!("body")];
    let question = checker
        .object(&value, &loc, None)
        .and_then(|obj| checker.string(obj, "question", &loc, true, Some(1), Some(4000), None));
    checker.finish()?;
    let question = question.expect("validated");
    on_manager(&app.handle, move |m| async move {
        run_dir_of(&m, &run_id)?;
        let reply = m.ask(&run_id, py_strip(&question)).await?;
        Ok(ok_json(&json!({"reply": reply})))
    })
    .await
}

pub async fn rebuild_report(State(app): State<AppState>, UrlPath(run_id): UrlPath<String>) -> ApiResult {
    on_manager(&app.handle, move |m| async move {
        run_dir_of(&m, &run_id)?;
        m.rebuild_report(&run_id)?;
        Ok(ok_json(&json!({"ok": true})))
    })
    .await
}

pub async fn list_seeds(State(app): State<AppState>, UrlPath(run_id): UrlPath<String>) -> ApiResult {
    on_manager(&app.handle, move |m| async move {
        let folder = run_dir_of(&m, &run_id)?.join("seeds");
        let mut files: Vec<(String, u64)> = std::fs::read_dir(&folder)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_file())
                    .map(|e| (e.file_name().to_string_lossy().into_owned(), e.metadata().map_or(0, |m| m.len())))
                    .filter(|(name, _)| !name.starts_with('.'))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        let out: Vec<Value> = files.into_iter().map(|(name, size)| json!({"name": name, "size": size})).collect();
        Ok(ok_json(&out))
    })
    .await
}

pub async fn get_seed(
    State(app): State<AppState>,
    method: Method,
    UrlPath((run_id, name)): UrlPath<(String, String)>,
) -> ApiResult {
    let seed = name.clone();
    let path = app
        .handle
        .call(move |m| async move {
            let path = run_dir_of(&m, &run_id)?.join("seeds").join(&seed);
            let base = Path::new(&seed).file_name().map(|n| n.to_string_lossy().into_owned());
            if base.as_deref() != Some(seed.as_str()) || seed.starts_with('.') || !path.is_file() {
                return Err(ApiError::new(404, "no such seed document"));
            }
            Ok(path)
        })
        .await??;
    web::file_response(&path, &method, web::guess_type(&name), Some(&name), &[]).await
}

pub async fn report_html(State(app): State<AppState>, method: Method, UrlPath(run_id): UrlPath<String>) -> ApiResult {
    let path = app
        .handle
        .call(move |m| async move {
            let path = run_dir_of(&m, &run_id)?.join("report.html");
            if !path.is_file() {
                return Err(ApiError::new(404, "this run has no report yet"));
            }
            Ok(path)
        })
        .await??;
    web::file_response(&path, &method, "text/html; charset=utf-8", None, &[("cache-control", "no-cache")]).await
}

/// `report.md`, the report as Markdown (for scripts and the MCP server; the app reads the JSON in the run detail).
pub async fn report_markdown(
    State(app): State<AppState>,
    method: Method,
    UrlPath(run_id): UrlPath<String>,
) -> ApiResult {
    let path = app
        .handle
        .call(move |m| async move {
            let path = run_dir_of(&m, &run_id)?.join("report.md");
            if !path.is_file() {
                return Err(ApiError::new(404, "this run has no report yet"));
            }
            Ok(path)
        })
        .await??;
    web::file_response(&path, &method, "text/markdown; charset=utf-8", None, &[("cache-control", "no-cache")]).await
}

// ---------- .tellfold bundles ----------

fn bundle_error(e: tellfold_study::Error) -> ApiError {
    match e {
        tellfold_study::Error::Bundle(m) | tellfold_study::Error::Example(m) => ApiError::new(400, m),
        other => ApiError::internal(other),
    }
}

/// Runs bundle work on a blocking worker, never on the manager thread that drives live runs and answers every other
/// request. The test hook, when set, runs first on that worker.
async fn bundle_work<T: Send + 'static>(
    app: &AppState,
    work: impl FnOnce() -> Result<T, tellfold_study::Error> + Send + 'static,
) -> Result<T, ApiError> {
    let hook = app.bundle_hook.clone();
    tokio::task::spawn_blocking(move || {
        if let Some(hook) = hook {
            (hook.0)();
        }
        work()
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(bundle_error)
}

/// Marks a run as being exported for as long as it lives; dropping it tells the manager the export ended. It is moved
/// into the blocking job, so it ends with the job even when the client goes away first.
struct ExportGuard {
    handle: Handle,
    run_id: String,
}

impl Drop for ExportGuard {
    fn drop(&mut self) {
        let run_id = std::mem::take(&mut self.run_id);
        self.handle.spawn(move |m| m.end_export(&run_id));
    }
}

/// The media type of a bundle, as the Mac app declares it.
pub const BUNDLE_TYPE: &str = "application/vnd.tellfold.run+zip";

/// The finished run as one `.tellfold` file (a zip of its cleaned folder) to send to someone else. The manager only
/// checks that the run may be shared and marks it as being exported (no report rebuild starts until the bundle is
/// written); the bundle is written to an unnamed temporary file off the manager thread and streamed from there.
pub async fn export_bundle(State(app): State<AppState>, UrlPath(run_id): UrlPath<String>, method: Method) -> ApiResult {
    let id = run_id.clone();
    let dir = app
        .handle
        .call(move |m| async move {
            let dir = run_dir_of(&m, &id)?;
            if m.is_live(&id) {
                return Err(ApiError::new(409, "the run is still in progress; share it once it has finished"));
            }
            if m.rebuild_state(&id).running {
                return Err(ApiError::new(409, "the report is being rebuilt; share the run once it is done"));
            }
            // checked and marked in one manager job, so a rebuild cannot start in between
            m.begin_export(&id);
            Ok(dir)
        })
        .await??;
    let guard = ExportGuard { handle: app.handle.clone(), run_id: run_id.clone() };
    let name = format!("{run_id}.{}", tellfold_study::bundle::EXTENSION);
    let file = bundle_work(&app, move || {
        let _guard = guard;
        tellfold_study::bundle::bundle_file(&dir, &run_id)
    })
    .await?;
    web::open_file_response(tokio::fs::File::from_std(file), &method, BUNDLE_TYPE, Some(&name), &[]).await
}

/// The request body in an unnamed temporary file, read from its start; refused when empty or over `limit` bytes.
async fn spool_body(body: Body, limit: u64) -> Result<std::fs::File, ApiError> {
    use futures::StreamExt;
    use tokio::io::{AsyncSeekExt, AsyncWriteExt};
    let mut file = tokio::fs::File::from_std(tempfile::tempfile().map_err(ApiError::internal)?);
    let mut stream = body.into_data_stream();
    let mut total: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ApiError::new(400, "the upload did not finish"))?;
        total += chunk.len() as u64;
        if total > limit {
            return Err(ApiError::new(413, format!("the bundle is larger than {} MB", limit / (1024 * 1024))));
        }
        file.write_all(&chunk).await.map_err(ApiError::internal)?;
    }
    if total == 0 {
        return Err(ApiError::new(400, "send the .tellfold file as the request body"));
    }
    file.flush().await.map_err(ApiError::internal)?;
    file.rewind().await.map_err(ApiError::internal)?;
    Ok(file.into_std().await)
}

/// Opens a `.tellfold` file sent as the request body: a new, read-only run in the runs folder. `{"id": new run id}`.
/// The body goes to a temporary file as it arrives and is unpacked and checked off the manager thread.
pub async fn import_bundle(State(app): State<AppState>, body: Body) -> ApiResult {
    let file = spool_body(body, tellfold_study::bundle::MAX_BUNDLE_BYTES).await?;
    let runs_dir = app.handle.call(|m| async move { m.runs_dir.clone() }).await?;
    let id = bundle_work(&app, move || tellfold_study::bundle::import_bundle(file, &runs_dir)).await?;
    Ok(ok_json(&json!({"id": id})))
}

/// Unsubscribes its client when the response stream is dropped (the client went away, or the stream ended).
struct Subscription {
    handle: Handle,
    run_id: String,
    queue: Arc<Queue>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let (run_id, queue) = (self.run_id.clone(), self.queue.clone());
        self.handle.spawn(move |m| m.unsubscribe(&run_id, &queue));
    }
}

pub async fn stream(State(app): State<AppState>, UrlPath(run_id): UrlPath<String>) -> ApiResult {
    let id = run_id.clone();
    let queue = app
        .handle
        .call(move |m| async move {
            run_dir_of(&m, &id)?;
            Ok::<_, ApiError>(m.subscribe(&id))
        })
        .await??;
    let subscription = Subscription { handle: app.handle.clone(), run_id, queue };
    let ping = Duration::from_secs_f64(app.ping_seconds);
    let body = futures::stream::unfold((Some(subscription), true), move |(sub, first)| async move {
        let sub = sub?;
        if first {
            return Some((
                Ok::<_, std::convert::Infallible>(Bytes::from_static(b": connected\n\n")),
                (Some(sub), false),
            ));
        }
        match tokio::time::timeout(ping, sub.queue.get()).await {
            Err(_) => Some((Ok(Bytes::from_static(b": ping\n\n")), (Some(sub), false))),
            Ok(None) => None, // server shutting down
            Ok(Some((name, data))) => {
                Some((Ok(Bytes::from(format!("event: {name}\ndata: {data}\n\n"))), (Some(sub), false)))
            }
        }
    });
    let mut response = Response::new(Body::from_stream(body));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(response)
}

// ---------- studies ----------

fn study_or_404(m: &Manager, study_id: &str) -> Result<Study, ApiError> {
    if !py_match(study_id_pattern(), study_id) || study_id.contains("..") {
        return Err(ApiError::new(400, "invalid study id"));
    }
    m.study(study_id).ok_or_else(|| ApiError::new(404, "study not found"))
}

fn kind_value(kind: StudyKind) -> &'static str {
    match kind {
        StudyKind::Compare => "compare",
        StudyKind::Repeat => "repeat",
    }
}

pub async fn list_studies(State(app): State<AppState>) -> ApiResult {
    on_manager(&app.handle, |m| async move {
        let mut out: Vec<Value> = Vec::new();
        for study in tellfold_store::study::list_studies(&m.runs_dir) {
            let members = m.study_members(&study);
            let Some(first) = members.first() else { continue };
            let question = m.reader(&first.run_id).config().map(|c| c.question).unwrap_or_default();
            let has_compare = if study.kind == StudyKind::Repeat {
                m.agreement(&study.id).is_some()
            } else {
                m.compare_result(&study.id).is_some()
            };
            let runs: Vec<Value> =
                members.iter().map(|x| json!({"id": x.run_id, "variant": x.variant, "letter": x.letter})).collect();
            out.push(json!({
                "id": study.id, "kind": kind_value(study.kind), "created_at": study.created_at, "question": question,
                "runs": runs, "has_compare": has_compare,
                "comparing": m.is_comparing(&study.id) || m.is_repeating(&study.id), "planned": study.planned,
            }));
        }
        out.sort_by(|a, b| {
            let key = |v: &Value| {
                (v["created_at"].as_str().unwrap_or("").to_string(), v["id"].as_str().unwrap_or("").to_string())
            };
            key(b).cmp(&key(a))
        });
        Ok(ok_json(&out))
    })
    .await
}

pub async fn get_study(State(app): State<AppState>, UrlPath(study_id): UrlPath<String>) -> ApiResult {
    on_manager(&app.handle, move |m| async move {
        let study = study_or_404(&m, &study_id)?;
        let mut runs: Vec<Value> = Vec::new();
        for member in m.study_members(&study) {
            let Some(summary) = m.reader(&member.run_id).summary(m.is_live(&member.run_id)) else { continue };
            let digest = digest_run(&member, &summary.state).map_err(ApiError::internal)?;
            let read_only = m.read_only(&member.run_id);
            let mut row = json!({
                "id": member.run_id, "variant": member.variant, "letter": member.letter,
                "question": summary.question, "state": summary.state, "has_report": summary.has_report,
                "example": read_only == Some(ReadOnly::Example),
                "digest": serde_json::to_value(&digest).map_err(ApiError::internal)?,
            });
            // only when set, as in the run list
            if read_only == Some(ReadOnly::Imported) {
                row["imported"] = json!(true);
            }
            runs.push(row);
        }
        let compare = m.compare_result(&study.id).map(|c| serde_json::to_value(c).unwrap_or(Value::Null));
        let agreement = m.agreement(&study.id).map(|a| serde_json::to_value(a).unwrap_or(Value::Null));
        Ok(ok_json(&json!({
            "id": study.id, "kind": kind_value(study.kind), "created_at": study.created_at, "runs": runs,
            "compare": compare, "compare_state": m.compare_state(&study.id), "planned": study.planned,
            "repeating": m.is_repeating(&study.id), "agreement": agreement,
        })))
    })
    .await
}

pub async fn branch(State(app): State<AppState>, UrlPath(run_id): UrlPath<String>, body: Bytes) -> ApiResult {
    let value = parse_body(&body, true)?.unwrap_or(Value::Null);
    let mut checker = Checker::new();
    let loc = [json!("body")];
    let fields = checker.object(&value, &loc, None).map(|obj| {
        let round = checker.int(obj, "round", &loc, None, Some(0), None);
        let events = checker.list(obj, "events", &loc, Some(50)).map(|items| {
            items
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    event_in(&mut checker, item, &[json!("body"), json!("events"), json!(i)], Some("EventIn"))
                })
                .collect::<Vec<_>>()
        });
        (round, events)
    });
    checker.finish()?;
    let Some((Some(round), Some(events))) = fields else { unreachable!("validated") };
    // what happens after `round`
    let events: Vec<GodEvent> = events
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let content = py_strip(&e.content);
            (!content.is_empty()).then(|| GodEvent::new(e.round, content))
        })
        .collect();
    on_manager(&app.handle, move |m| async move {
        run_dir_of(&m, &run_id)?;
        let (id, study_id) = m.branch(&run_id, round, events).await?;
        Ok(ok_json(&json!({"id": id, "study_id": study_id})))
    })
    .await
}

pub async fn rewrite(State(app): State<AppState>, UrlPath(run_id): UrlPath<String>) -> ApiResult {
    on_manager(&app.handle, move |m| async move {
        run_dir_of(&m, &run_id)?;
        Ok(ok_json(&m.rewrite(&run_id).await?))
    })
    .await
}

pub async fn saved_rewrite(State(app): State<AppState>, UrlPath(run_id): UrlPath<String>) -> ApiResult {
    on_manager(&app.handle, move |m| async move {
        run_dir_of(&m, &run_id)?;
        match m.saved_rewrite(&run_id) {
            Some(suggestion) => Ok(ok_json(&suggestion)),
            None => Err(ApiError::new(404, "no suggestion for this run yet")),
        }
    })
    .await
}

pub async fn check_run(State(app): State<AppState>, UrlPath(run_id): UrlPath<String>, body: Bytes) -> ApiResult {
    let value = parse_body(&body, true)?.unwrap_or(Value::Null);
    let mut checker = Checker::new();
    let loc = [json!("body")];
    let fields = checker.object(&value, &loc, None).map(|obj| {
        let runs = checker.int(obj, "runs", &loc, Some(3), Some(2), Some(5));
        let model = checker.string(obj, "agent_model", &loc, false, None, Some(120), Some(model_pattern()));
        (runs, model)
    });
    checker.finish()?;
    let Some((Some(runs), agent_model)) = fields else { unreachable!("validated") };
    on_manager(&app.handle, move |m| async move {
        run_dir_of(&m, &run_id)?;
        let study = m.check(&run_id, runs, agent_model).await?;
        Ok(ok_json(&json!({"study_id": study.id})))
    })
    .await
}

pub async fn compare_study(State(app): State<AppState>, UrlPath(study_id): UrlPath<String>) -> ApiResult {
    on_manager(&app.handle, move |m| async move {
        study_or_404(&m, &study_id)?;
        m.compare(&study_id)?.ok_or_else(|| ApiError::new(404, "study not found"))?;
        Ok(ok_json(&json!({"ok": true})))
    })
    .await
}

// ---------- runtimes and settings ----------

/// 400 for a malformed or unknown runtime id; returns the id.
pub fn check_runtime_id(runtime_id: &str, settings: &Settings, field: &str) -> Result<String, ApiError> {
    let runtime_id = py_strip(runtime_id);
    if !py_match(runtime_id_pattern(), runtime_id) {
        return Err(ApiError::new(400, format!("invalid {field}")));
    }
    if !known_ids(settings).iter().any(|k| k == runtime_id) {
        let quoted = tellfold_models::json::py_repr(&Value::String(runtime_id.into()));
        return Err(ApiError::new(400, format!("unknown {field} {quoted}; see GET /api/runtimes")));
    }
    Ok(runtime_id.to_string())
}

pub async fn list_runtimes(State(app): State<AppState>, Query(query): Query<Vec<(String, String)>>) -> ApiResult {
    let check_auth = query_int(last(&query, "check_auth").as_deref(), "check_auth", Some(0), Some(0), Some(1))?;
    let settings = load_settings(app.settings_path.as_deref());
    Ok(ok_json(&detect_all(&settings, check_auth == Some(1)).await))
}

pub async fn get_settings(State(app): State<AppState>) -> Response {
    ok_json(&load_settings(app.settings_path.as_deref()))
}

/// pydantic's error for a settings field, from the "field: message" text of [`Settings`]' validation.
fn settings_error(message: &str, body: &Value) -> Value {
    let (field, msg) = match message.split_once(": ") {
        Some((field, msg)) if !field.contains(' ') => (Some(field), msg),
        _ => (None, message),
    };
    let mut loc = vec![json!("body")];
    if let Some(field) = field {
        loc.push(json!(field));
    }
    let input = field.and_then(|f| body.get(f)).cloned().unwrap_or_else(|| body.clone());
    let (kind, msg) = if msg.starts_with("Input should be a valid string") {
        ("string_type", msg.to_string())
    } else if msg.starts_with("Input should be a valid integer") {
        ("int_parsing", msg.to_string())
    } else if msg.starts_with("Input should be a valid boolean") {
        ("bool_parsing", msg.to_string())
    } else if msg.starts_with("Input should be greater than or equal") {
        ("greater_than_equal", msg.to_string())
    } else if msg.starts_with("Input should be less than or equal") {
        ("less_than_equal", msg.to_string())
    } else if msg.starts_with("Field required") {
        ("missing", msg.to_string())
    } else if msg.starts_with("Input should be") {
        ("type_error", msg.to_string())
    } else if msg.starts_with("Value error, ") {
        ("value_error", msg.to_string())
    } else {
        ("value_error", format!("Value error, {msg}"))
    };
    let bound = || msg.rsplit(' ').next().and_then(|n| n.parse::<i64>().ok());
    let ctx = match kind {
        "value_error" => Some(json!({"error": {}})),
        "greater_than_equal" => bound().map(|n| json!({"ge": n})),
        "less_than_equal" => bound().map(|n| json!({"le": n})),
        _ => None,
    };
    let mut item = json!({"type": kind, "loc": loc, "msg": msg, "input": input});
    if let Some(ctx) = ctx {
        item["ctx"] = ctx;
    }
    item
}

pub async fn put_settings(State(app): State<AppState>, body: Bytes) -> ApiResult {
    let value = parse_body(&body, true)?.unwrap_or(Value::Null);
    if !value.is_object() {
        let mut checker = Checker::new();
        checker.object(&value, &[json!("body")], None);
        checker.finish()?;
    }
    let settings: Settings = serde_json::from_value(value.clone())
        .map_err(|e| ApiError::validation(vec![settings_error(&e.to_string(), &value)]))?;
    let settings = settings.validated().map_err(|e| ApiError::validation(vec![settings_error(&e, &value)]))?;
    check_runtime_id(&settings.agent_runtime, &settings, "agent_runtime")?;
    check_runtime_id(&settings.planner_runtime, &settings, "planner_runtime")?;
    if let Err(e) = save_settings(&settings, app.settings_path.as_deref()) {
        return Err(ApiError::new(500, format!("could not save settings: {}", crate::upload::strerror(&e))));
    }
    Ok(ok_json(&settings))
}

pub async fn test_runtime_route(
    State(app): State<AppState>,
    UrlPath(runtime_id): UrlPath<String>,
    body: Bytes,
) -> ApiResult {
    let value = parse_body(&body, false)?;
    let mut checker = Checker::new();
    let loc = [json!("body")];
    let model = match &value {
        None | Some(Value::Null) => None,
        Some(v) => checker
            .object(v, &loc, None)
            .and_then(|obj| checker.string(obj, "model", &loc, false, None, Some(120), None)),
    };
    checker.finish()?;
    if !py_match(runtime_id_pattern(), &runtime_id) {
        return Err(ApiError::new(400, "invalid runtime id"));
    }
    let model = model.map(|m| py_strip(&m).to_string()).filter(|m| !m.is_empty());
    if model.as_deref().is_some_and(|m| !py_match(model_pattern(), m)) {
        return Err(ApiError::new(400, "invalid model"));
    }
    let settings = load_settings(app.settings_path.as_deref());
    if !known_ids(&settings).contains(&runtime_id) {
        return Err(ApiError::new(404, "runtime not found"));
    }
    if app.fake {
        return Ok(ok_json(
            &json!({"ok": true, "seconds": 0.0, "reply": "ok (fake LLM)", "error": null, "model": model}),
        ));
    }
    match test_runtime(&runtime_id, &settings, model.as_deref(), Role::Agent, 120.0).await {
        Ok(result) => Ok(ok_json(&result)),
        Err(_) => Err(ApiError::new(404, "runtime not found")),
    }
}

// ---------- frontend and fallbacks ----------

pub async fn static_file(method: Method, UrlPath(path): UrlPath<String>) -> Response {
    web::static_response(&path, &method)
}

/// Every path no route took: `/api/...` is a JSON 404, a GET of anything else is the single-page app.
pub async fn fallback(method: Method, uri: axum::http::Uri) -> Response {
    let path = percent_decode(uri.path());
    let api = path == "/api" || path.starts_with("/api/");
    let read = method == Method::GET || method == Method::HEAD;
    if api && (read || [Method::POST, Method::PUT, Method::PATCH, Method::DELETE].contains(&method)) {
        return ApiError::new(404, "not found").into_response();
    }
    if !read {
        return ApiError::new(405, "Method Not Allowed").into_response();
    }
    if path == "/static" || path.starts_with("/static/") {
        return web::static_response(path.trim_start_matches("/static").trim_start_matches('/'), &method);
    }
    web::index_response(&method)
}

pub fn percent_decode(path: &str) -> String {
    percent_encoding::percent_decode_str(path).decode_utf8_lossy().into_owned()
}

// ---------- guards ----------

/// The host name in a Host header, as `urlsplit("//" + host).hostname` reads it ("" when there is none).
pub fn hostname(host: &str) -> String {
    let info = host.rsplit_once('@').map_or(host, |(_, rest)| rest);
    let name = match info.split_once('[') {
        Some((_, bracketed)) => match bracketed.split_once(']') {
            Some((inside, _)) => inside,
            None => return String::new(),
        },
        None => info.split(':').next().unwrap_or(""),
    };
    name.to_lowercase()
}

/// `urlsplit(origin).netloc`.
fn netloc(url: &str) -> String {
    let rest = match url.split_once(':') {
        Some((scheme, rest))
            if !scheme.is_empty()
                && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) =>
        {
            rest
        }
        _ => url,
    };
    match rest.strip_prefix("//") {
        Some(after) => after.split(['/', '?', '#']).next().unwrap_or("").to_string(),
        None => String::new(),
    }
}

/// Reject requests whose Host is not local (DNS rebinding) and writes whose Origin is another site.
pub fn local_only(allowed: &HashSet<String>, method: &Method, headers: &HeaderMap) -> Option<Response> {
    let host = headers.get_all(header::HOST).iter().next_back().map(|v| latin1(v.as_bytes())).unwrap_or_default();
    if !allowed.contains(&hostname(&host)) {
        return Some(ApiError::new(403, "forbidden host").into_response());
    }
    if let Some(origin) = headers.get_all(header::ORIGIN).iter().next_back().map(|v| latin1(v.as_bytes())) {
        if ![Method::GET, Method::HEAD, Method::OPTIONS].contains(method)
            && (origin == "null" || netloc(&origin).to_lowercase() != host.to_lowercase())
        {
            return Some(ApiError::new(403, "cross-origin request refused").into_response());
        }
    }
    None
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|b| *b as char).collect()
}

/// Constant-time comparison of the whole `Authorization` header with `Bearer <token>`.
pub fn token_ok(expected: &[u8], headers: &HeaderMap) -> bool {
    let supplied = headers.get(header::AUTHORIZATION).map(|v| v.as_bytes()).unwrap_or(b"");
    let mut diff = supplied.len() ^ expected.len();
    for (i, byte) in expected.iter().enumerate() {
        diff |= (*byte ^ supplied.get(i).copied().unwrap_or(0)) as usize;
    }
    diff == 0
}

pub fn unauthorized() -> Response {
    ApiError::new(401, "unauthorized").with_header(header::WWW_AUTHENTICATE, "Bearer").into_response()
}
