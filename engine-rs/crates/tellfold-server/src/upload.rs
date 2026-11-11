//! `POST /api/runs`: the multipart form of a new run (seed files and settings), checked in the order and with the
//! messages of `create_run` in `tellfold/server/app.py`.

use std::collections::HashSet;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use axum::extract::multipart::MultipartError;
use axum::extract::{FromRequest, Multipart, Request};
use axum::http::header;
use regex::Regex;
use serde_json::{json, Value};
use unicode_normalization::UnicodeNormalization;

use tellfold_engine::personas::tidy_cast;
use tellfold_models::text::{py_split, py_strip};
use tellfold_models::{GodEvent, KnowledgeGraph, Persona, SimConfig};
use tellfold_runtimes::load_settings;
use tellfold_store::files::{now_iso, write_json, write_json_list, GRAPH_FILE, PERSONAS_FILE};
use tellfold_store::study::{join_study, plan_variant};

use crate::error::{ok_json, ApiError, ApiResult};
use crate::manager::Manager;
use crate::routes::{check_runtime_id, model_pattern, py_match, run_id_pattern, study_id_pattern};
use crate::AppState;

pub const SEED_SUFFIXES: [&str; 4] = [".pdf", ".md", ".markdown", ".txt"];
pub const MAX_FILE_BYTES: usize = 20 * 1024 * 1024;
/// a whole folder of project docs
pub const MAX_FILES: usize = 100;
pub const MAX_FIELDS: usize = 100;
pub const MAX_TOTAL_BYTES: usize = 100 * 1024 * 1024;
pub const MAX_FORM_BYTES: usize = MAX_TOTAL_BYTES + 1024 * 1024;

/// Bytes an uploaded file part may keep in memory before the rest goes to a temporary file, like Starlette's
/// `SpooledTemporaryFile`.
const SPOOL_BYTES: usize = 1024 * 1024;

/// An uploaded file's contents, read as they arrive.
#[derive(Debug)]
pub struct Upload {
    pub size: usize,
    file: Mutex<tempfile::SpooledTempFile>,
}

impl Upload {
    /// Copies the contents to `target`.
    fn save(&self, target: &Path) -> std::io::Result<()> {
        let mut file = self.file.lock().expect("not poisoned");
        file.rewind()?;
        let mut out = std::fs::File::create(target)?;
        std::io::copy(&mut *file, &mut out)?;
        Ok(())
    }
}

/// One part of the form: a text field, or a file (a part with a file name).
#[derive(Debug, Clone)]
pub enum FormItem {
    Text(String),
    File { filename: String, data: Arc<Upload> },
}

/// The parsed form: items in the order sent; `get` returns the last value of a key, as Starlette's form does.
#[derive(Debug, Default)]
pub struct Form {
    pub items: Vec<(String, FormItem)>,
}

impl Form {
    pub fn get(&self, key: &str) -> Option<&FormItem> {
        self.items.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn text(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(FormItem::Text(t)) => Some(t),
            _ => None,
        }
    }
}

/// `e.strerror or e`: the OS message without Rust's "(os error N)".
pub fn strerror(e: &std::io::Error) -> String {
    let text = e.to_string();
    match text.rfind(" (os error ") {
        Some(i) => text[..i].to_string(),
        None => text,
    }
}

fn unsafe_name() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[^A-Za-z0-9._ -]+").expect("valid pattern"))
}

/// A plain file name for an upload: no directories, no odd characters, no hidden files, unique.
pub fn safe_filename(name: &str, taken: &mut HashSet<String>) -> String {
    let replaced = name.replace('\\', "/");
    let base = replaced.rsplit('/').next().unwrap_or("");
    let base: String = base.nfkd().filter(char::is_ascii).collect();
    let (stem, suffix) = match base.rfind('.') {
        Some(i) => (&base[..i], &base[i + 1..]),
        None => (base.as_str(), ""),
    };
    let stem = unsafe_name().replace_all(stem, "_");
    let stem: String = stem.trim_matches(|c| " ._-".contains(c)).chars().take(80).collect();
    let stem = if stem.is_empty() { "seed".to_string() } else { stem };
    let suffix = suffix.to_lowercase();
    let mut candidate = format!("{stem}.{suffix}");
    let mut n = 2;
    while taken.contains(&candidate.to_lowercase()) {
        candidate = format!("{stem}-{n}.{suffix}");
        n += 1;
    }
    taken.insert(candidate.to_lowercase());
    candidate
}

/// `Path(name).suffix`: the last dotted part of the final component, "" for a hidden file like ".md".
pub fn py_suffix(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or("");
    match base.rfind('.') {
        Some(i) if i > 0 && i < base.len() - 1 => base[i..].to_string(),
        _ => String::new(),
    }
}

/// Python's `repr()` of a string.
fn repr(text: &str) -> String {
    tellfold_models::json::py_repr(&Value::String(text.into()))
}

/// `int(text)`: surrounding whitespace, a sign and underscores between digits.
fn int_field(form: &Form, key: &str, default: i64, low: i64, high: i64) -> Result<i64, ApiError> {
    let raw = match form.get(key) {
        None => return Ok(default),
        Some(FormItem::Text(t)) if py_strip(t).is_empty() => return Ok(default),
        Some(FormItem::Text(t)) => crate::validate::parse_py_int(t),
        Some(FormItem::File { .. }) => None,
    };
    let value = raw.ok_or_else(|| ApiError::new(400, format!("{key} must be a whole number")))?;
    if !(low..=high).contains(&value) {
        return Err(ApiError::new(400, format!("{key} must be between {low} and {high}")));
    }
    Ok(value)
}

fn str_field(
    form: &Form,
    key: &str,
    default: &str,
    max_len: usize,
    pattern: Option<&Regex>,
) -> Result<String, ApiError> {
    let Some(raw) = form.text(key).filter(|t| !py_strip(t).is_empty()) else { return Ok(default.into()) };
    let value = py_strip(raw);
    if value.chars().count() > max_len || pattern.is_some_and(|p| !py_match(p, value)) {
        return Err(ApiError::new(400, format!("invalid {key}")));
    }
    Ok(value.into())
}

fn bool_field(form: &Form, key: &str) -> bool {
    form.text(key).is_some_and(|t| matches!(py_strip(t).to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

/// `GodEvent.model_validate(item)` in lax mode: an int-like round and a string content.
fn god_event(item: &Value) -> Option<GodEvent> {
    let obj = item.as_object()?;
    let round = match obj.get("round")? {
        Value::Bool(b) => *b as i64,
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))?,
        Value::String(s) => crate::validate::parse_py_int(s)?,
        _ => return None,
    };
    let content = obj.get("content")?.as_str()?;
    Some(GodEvent::new(round, content))
}

fn events_field(form: &Form, rounds: i64) -> Result<Vec<GodEvent>, ApiError> {
    let Some(raw) = form.text("events").filter(|t| !py_strip(t).is_empty()) else { return Ok(Vec::new()) };
    let bad = || ApiError::new(400, "events must be a JSON list of {round, content}");
    let items: Vec<Value> = match serde_json::from_str::<Value>(raw) {
        Ok(Value::Array(items)) => items,
        _ => return Err(bad()),
    };
    let events = items.iter().map(god_event).collect::<Option<Vec<_>>>().ok_or_else(bad)?;
    let mut out = Vec::new();
    for e in events {
        let content = py_strip(&e.content);
        if content.is_empty() {
            continue;
        }
        if !(1..=rounds).contains(&e.round) {
            return Err(ApiError::new(400, format!("event round {} is outside 1..{rounds}", e.round)));
        }
        if content.chars().count() > 2000 {
            return Err(ApiError::new(400, "event text is too long (max 2000 characters)"));
        }
        out.push(GodEvent::new(e.round, content));
    }
    Ok(out)
}

async fn read_form(request: Request, app: &AppState) -> Result<Form, ApiError> {
    let content_type =
        request.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
    let mut form = Form::default();
    if content_type.starts_with("multipart/form-data") {
        let mut multipart = Multipart::from_request(request, app)
            .await
            .map_err(|e| ApiError::new(400, format!("could not read the form: {}", e.body_text())))?;
        let (mut files, mut fields) = (0, 0);
        loop {
            let mut field = match multipart.next_field().await {
                Ok(Some(field)) => field,
                Ok(None) => break,
                Err(e) => return Err(form_error(e)),
            };
            let key = field.name().unwrap_or("").to_string();
            let filename = field.file_name().map(str::to_string);
            match filename {
                Some(filename) => {
                    let mut spool = tempfile::SpooledTempFile::new(SPOOL_BYTES);
                    let mut size = 0;
                    while let Some(chunk) = field.chunk().await.map_err(form_error)? {
                        size += chunk.len();
                        spool.write_all(&chunk).map_err(ApiError::internal)?;
                    }
                    let data = Arc::new(Upload { size, file: Mutex::new(spool) });
                    files += 1;
                    if files > MAX_FILES {
                        return Err(ApiError::new(
                            400,
                            format!("Too many files. Maximum number of files is {MAX_FILES}."),
                        ));
                    }
                    form.items.push((key, FormItem::File { filename, data }));
                }
                None => {
                    let data = field.bytes().await.map_err(form_error)?;
                    fields += 1;
                    if fields > MAX_FIELDS {
                        return Err(ApiError::new(
                            400,
                            format!("Too many fields. Maximum number of fields is {MAX_FIELDS}."),
                        ));
                    }
                    form.items.push((key, FormItem::Text(String::from_utf8_lossy(&data).into_owned())));
                }
            }
        }
    } else if content_type.starts_with("application/x-www-form-urlencoded") {
        let body = axum::body::to_bytes(request.into_body(), MAX_FORM_BYTES)
            .await
            .map_err(|e| ApiError::new(400, format!("could not read the form: {e}")))?;
        for (k, v) in url_pairs(&body) {
            form.items.push((k, FormItem::Text(v)));
        }
    }
    Ok(form)
}

/// A broken form is a 400; one over the body limit (a chunked upload has no Content-Length to refuse up front) is
/// the 413 `_save_uploads` gives for too much in total.
fn form_error(e: MultipartError) -> ApiError {
    if e.status() == axum::http::StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::new(413, "the seed files add up to more than 100 MB")
    } else {
        ApiError::new(400, format!("could not read the form: {}", e.body_text()))
    }
}

fn url_pairs(body: &[u8]) -> Vec<(String, String)> {
    let decode = |s: &str| percent_encoding::percent_decode_str(&s.replace('+', " ")).decode_utf8_lossy().into_owned();
    String::from_utf8_lossy(body)
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (decode(k), decode(v))
        })
        .collect()
}

/// Everything the form says about the run, before any file is written.
struct Planned {
    config: SimConfig,
    uploads: Vec<(String, Arc<Upload>)>,
    cast_from: String,
    study_id: Option<String>,
    variant: Option<String>,
    audience: Option<Audience>,
}

/// The `audience` field: a saved cast the app sends to skip the graph and persona stages.
#[derive(serde::Deserialize)]
struct Audience {
    name: String,
    personas: Vec<Persona>,
    #[serde(default)]
    graph: KnowledgeGraph,
}

/// Longest audience name kept in `config.audience`.
const AUDIENCE_NAME_MAX: usize = 80;

fn audience_field(form: &Form) -> Result<Option<Audience>, ApiError> {
    let Some(text) = form.text("audience").filter(|t| !py_strip(t).is_empty()) else { return Ok(None) };
    let mut audience: Audience =
        serde_json::from_str(text).map_err(|e| ApiError::new(400, format!("audience is not a valid cast: {e}")))?;
    let name = py_split(&audience.name).collect::<Vec<_>>().join(" ");
    audience.name =
        if name.is_empty() { "Saved audience".into() } else { name.chars().take(AUDIENCE_NAME_MAX).collect() };
    audience.personas = tidy_cast(audience.personas);
    if audience.personas.is_empty() {
        return Err(ApiError::new(400, "the audience has no people"));
    }
    Ok(Some(audience))
}

fn plan(form: &Form, app: &AppState) -> Result<Planned, ApiError> {
    let uploads: Vec<(String, Arc<Upload>)> = ["files[]", "files"]
        .iter()
        .flat_map(|key| {
            form.items.iter().filter(move |(k, _)| k == key).filter_map(|(_, item)| match item {
                FormItem::File { filename, data } => Some((filename.clone(), data.clone())),
                FormItem::Text(_) => None,
            })
        })
        .collect();
    if uploads.is_empty() {
        return Err(ApiError::new(400, "add at least one seed file (.pdf, .md, .markdown or .txt)"));
    }
    if uploads.len() > MAX_FILES {
        return Err(ApiError::new(400, format!("at most {MAX_FILES} seed files")));
    }
    for (filename, data) in &uploads {
        let suffix = py_suffix(&filename.replace('\\', "/")).to_lowercase();
        if !SEED_SUFFIXES.contains(&suffix.as_str()) {
            return Err(ApiError::new(
                400,
                format!("{}: only .pdf, .md, .markdown and .txt are supported", repr(filename)),
            ));
        }
        if data.size > MAX_FILE_BYTES {
            return Err(ApiError::new(413, format!("{filename} is larger than 20 MB")));
        }
    }
    let Some(question) = form.text("question").filter(|q| !py_strip(q).is_empty()) else {
        return Err(ApiError::new(400, "question is required"));
    };
    let question = py_split(question).collect::<Vec<_>>().join(" ");
    if question.chars().count() > 2000 {
        return Err(ApiError::new(400, "question is too long (max 2000 characters)"));
    }
    let rounds = int_field(form, "rounds", 6, 1, 50)?;
    let mut config = SimConfig::new(&question, Vec::new());
    config.num_agents = int_field(form, "agents", 20, 2, 200)?;
    config.rounds = rounds;
    config.batch_size = int_field(form, "batch", 6, 1, 20)?;
    config.agent_model = str_field(form, "agent_model", "haiku", 120, Some(model_pattern()))?;
    config.planner_model = str_field(form, "planner_model", "sonnet", 120, Some(model_pattern()))?;
    config.concurrency = int_field(form, "concurrency", 4, 1, 16)?;
    config.language = str_field(form, "language", "English", 40, None)?;
    config.think = bool_field(form, "think");
    config.events = events_field(form, rounds)?;
    config.seed = int_field(form, "seed", 7, 0, (1 << 31) - 1)?;
    let settings = load_settings(app.settings_path.as_deref());
    for (key, default) in
        [("agent_runtime", settings.agent_runtime.clone()), ("planner_runtime", settings.planner_runtime.clone())]
    {
        let value = form.text(key).map(py_strip).filter(|v| !v.is_empty()).unwrap_or(&default).to_string();
        let checked = check_runtime_id(&value, &settings, key)?;
        if key == "agent_runtime" {
            config.agent_runtime = checked;
        } else {
            config.planner_runtime = checked;
        }
    }
    let cast_from = str_field(form, "cast_from", "", 200, Some(run_id_pattern()))?;
    let study_id = Some(str_field(form, "study_id", "", 100, Some(study_id_pattern()))?).filter(|s| !s.is_empty());
    let variant = Some(str_field(form, "variant", "", 60, None)?).filter(|s| !s.is_empty());
    config.review_cast = bool_field(form, "review_cast");
    config.fair_turns = bool_field(form, "fair_turns");
    let audience = audience_field(form)?;
    if audience.is_some() && !cast_from.is_empty() {
        return Err(ApiError::new(400, "use either cast_from or audience, not both"));
    }
    Ok(Planned { config, uploads, cast_from, study_id, variant, audience })
}

/// The config of the run whose people a new run reuses; 400 unless it got as far as having people.
fn cast_source(m: &Manager, run_id: &str) -> Result<SimConfig, ApiError> {
    if run_id.contains("..") {
        return Err(ApiError::new(400, "invalid cast_from"));
    }
    let path = m.run_path(run_id);
    let resolve = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if !m.is_example(run_id) && resolve(&path).parent() != Some(resolve(&m.runs_dir).as_path()) {
        return Err(ApiError::new(400, "invalid cast_from"));
    }
    let Some(config) = m.reader(run_id).config() else {
        return Err(ApiError::new(400, format!("cast_from: there is no run {run_id}")));
    };
    if !path.join("personas.json").is_file() || !path.join("graph.json").is_file() {
        return Err(ApiError::new(400, format!("cast_from: {run_id} has no people yet")));
    }
    Ok(config)
}

fn save_uploads(uploads: &[(String, Arc<Upload>)], dest: &Path) -> Result<Vec<PathBuf>, ApiError> {
    std::fs::create_dir_all(dest).map_err(ApiError::internal)?;
    let mut taken = HashSet::new();
    let mut saved = Vec::new();
    let mut total = 0;
    for (filename, data) in uploads {
        let name = safe_filename(if filename.is_empty() { "seed" } else { filename }, &mut taken);
        let target = dest.join(&name);
        if data.size > MAX_FILE_BYTES {
            return Err(ApiError::new(413, format!("{filename} is larger than 20 MB")));
        }
        if total + data.size > MAX_TOTAL_BYTES {
            let _ = std::fs::write(&target, b"");
            return Err(ApiError::new(413, "the seed files add up to more than 100 MB"));
        }
        data.save(&target).map_err(ApiError::internal)?;
        if data.size == 0 {
            return Err(ApiError::new(400, format!("{filename} is empty")));
        }
        total += data.size;
        saved.push(target);
    }
    Ok(saved)
}

pub async fn create_run(app: AppState, request: Request) -> ApiResult {
    let length = request.headers().get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).unwrap_or("");
    if !length.is_empty()
        && length.bytes().all(|b| b.is_ascii_digit())
        && length.parse::<u128>().is_ok_and(|n| n > MAX_FORM_BYTES as u128)
    {
        return Err(ApiError::new(413, "upload too large"));
    }
    let form = read_form(request, &app).await?;
    let planned = plan(&form, &app)?;
    drop(form);
    app.handle
        .call(move |m| async move {
            let Planned { mut config, uploads, cast_from, study_id, variant, audience } = planned;
            let mut study = None;
            if !cast_from.is_empty() {
                let source = cast_source(&m, &cast_from)?;
                let (planned_study, label) = plan_variant(
                    &m.runs_dir,
                    &cast_from,
                    &source,
                    study_id.as_deref(),
                    variant.as_deref(),
                    &now_iso(),
                    None,
                )
                .map_err(|e| ApiError::new(400, e.to_string()))?;
                config.cast_from = Some(cast_from);
                config.study_id = Some(planned_study.id.clone());
                config.variant = Some(label);
                config.num_agents = source.num_agents;
                study = Some(planned_study);
            } else if study_id.is_some() || variant.is_some() {
                return Err(ApiError::new(
                    400,
                    "study_id and variant need cast_from: a study compares runs of one crowd",
                ));
            }
            let run_dir = m.create_run_dir(&config.question).map_err(ApiError::internal)?;
            let seeds = match save_uploads(&uploads, &run_dir.join("seeds")) {
                Ok(seeds) => seeds,
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&run_dir);
                    return Err(e);
                }
            };
            config.seed_files = seeds.iter().map(|p| p.to_string_lossy().into_owned()).collect();
            if let Some(audience) = audience {
                // the runner reads these back as the cast, so the graph and persona stages cost nothing
                let written = write_json(&run_dir.join(GRAPH_FILE), &audience.graph)
                    .and_then(|_| write_json_list(&run_dir.join(PERSONAS_FILE), &audience.personas));
                if let Err(e) = written {
                    let _ = std::fs::remove_dir_all(&run_dir);
                    return Err(ApiError::internal(e));
                }
                config.num_agents = audience.personas.len() as i64;
                config.audience = Some(audience.name);
            }
            let run_name = run_dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
            if let Some(study) = &study {
                join_study(&m.runs_dir, study, &run_name, config.variant.as_deref().unwrap_or(""))
                    .map_err(ApiError::internal)?;
            }
            let run_id = m.start(&run_dir, config, seeds)?;
            Ok(ok_json(&json!({"id": run_id})))
        })
        .await?
}
