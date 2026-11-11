//! Adapters for HTTP model servers: Ollama, any OpenAI-compatible server, the Anthropic and OpenAI APIs (port of
//! `tellfold/runtimes/http_api.py`).
//!
//! API keys are read from environment variables at call time and only ever placed in request headers; error
//! messages carry the status and a short body excerpt, never headers.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use regex::Regex;
use serde_json::{json, Map, Value};
use tellfold_llm::adapter::{Adapter, AdapterError, AdapterReply, AdapterRequest};
use tellfold_models::json::{py_repr, py_str};
use tellfold_models::text::{py_prefix, py_split, py_strip};

use crate::settings::renamed_key_env;

pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
pub const ANTHROPIC_VERSION: &str = "2026-10-21";
pub const ANTHROPIC_ALIASES: &[(&str, &str)] =
    &[("haiku", "claude-haiku-5-5"), ("sonnet", "claude-sonnet-5-5"), ("opus", "claude-opus-5-5")];
pub const ANTHROPIC_MODELS: &[&str] = &["claude-haiku-5-5", "claude-sonnet-5-5", "claude-opus-5-5"];
pub const ANTHROPIC_MAX_TOKENS: i64 = 16000;
pub const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
pub const OPENAI_MODELS: &[&str] = &["gpt-5-mini", "gpt-5", "gpt-5-nano"];
const DEFAULT_AUTH_HINT: &str = "The server rejected the API key.";
const DEFAULT_TIMEOUT_S: f64 = 240.0;

/// `" ".join(text.split())[:n]`.
fn squash(text: &str, n: usize) -> String {
    py_prefix(&py_split(text).collect::<Vec<_>>().join(" "), n).to_string()
}

fn truthy(value: Option<&Value>) -> Option<&Value> {
    value.filter(|v| match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    })
}

/// A short, header-free description of an error response body.
pub fn excerpt(body: &str) -> String {
    let Ok(data) = serde_json::from_str::<Value>(body) else { return squash(body, 300) };
    if let Value::Object(obj) = &data {
        match obj.get("error") {
            Some(Value::Object(err)) => {
                let shown = truthy(err.get("message"))
                    .or_else(|| truthy(err.get("type")))
                    .map(py_str)
                    .unwrap_or_else(|| py_str(&Value::Object(err.clone())));
                return py_prefix(&shown, 300).to_string();
            }
            Some(err) if truthy(Some(err)).is_some() => return py_prefix(&py_str(err), 300).to_string(),
            _ => {}
        }
        if let Some(message) = truthy(obj.get("message")) {
            return py_prefix(&py_str(message), 300).to_string();
        }
    }
    squash(&py_str(&data), 300)
}

/// Python's `int(data.get(key) or 0)` for a token count.
fn count(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        _ => 0,
    }
}

/// A failed request: BadRequest (HTTP 400/422, the server rejected the request shape) is a Fatal that some
/// adapters catch to fall back.
#[derive(Debug, Clone, PartialEq)]
pub enum RequestError {
    BadRequest(String),
    Adapter(AdapterError),
}

impl From<RequestError> for AdapterError {
    fn from(e: RequestError) -> AdapterError {
        match e {
            RequestError::BadRequest(m) => AdapterError::Fatal(m),
            RequestError::Adapter(e) => e,
        }
    }
}

/// What every HTTP adapter shares: base URL, key lookup, status mapping.
pub struct HttpClient {
    pub runtime_id: String,
    pub base_url: String,
    pub api_key_env: Option<String>,
    pub auth_hint: String,
    pub timeout_s: f64,
    client: reqwest::Client,
}

impl HttpClient {
    pub fn new(runtime_id: &str, base_url: &str, api_key_env: Option<&str>) -> HttpClient {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("an HTTP client with default TLS settings");
        HttpClient {
            runtime_id: runtime_id.into(),
            base_url: base_url.trim_end_matches('/').into(),
            api_key_env: api_key_env.map(str::to_string),
            auth_hint: DEFAULT_AUTH_HINT.into(),
            timeout_s: DEFAULT_TIMEOUT_S,
            client,
        }
    }

    pub fn api_key(&self) -> Option<String> {
        let name = self.api_key_env.as_deref()?;
        let value = std::env::var(name)
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| std::env::var(renamed_key_env(name)).ok())
            .unwrap_or_default();
        let value = py_strip(&value);
        (!value.is_empty()).then(|| value.to_string())
    }

    fn bearer(&self) -> Vec<(&'static str, String)> {
        self.api_key().map(|key| vec![("Authorization", format!("Bearer {key}"))]).unwrap_or_default()
    }

    pub async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        headers: &[(&'static str, String)],
        timeout_s: Option<f64>,
    ) -> Result<Value, RequestError> {
        let id = &self.runtime_id;
        let transient = |m: String| RequestError::Adapter(AdapterError::transient(m));
        let url = format!("{}{path}", self.base_url);
        let mut builder =
            self.client.request(method, &url).timeout(Duration::from_secs_f64(timeout_s.unwrap_or(self.timeout_s)));
        for (name, value) in headers {
            builder = builder.header(*name, value);
        }
        if let Some(body) = body {
            builder = builder.json(body);
        }
        let failed = |e: reqwest::Error, reading: bool| {
            let kind = httpx_error_name(&e, reading);
            if e.is_timeout() {
                transient(format!("{id}: request timed out ({kind})"))
            } else {
                transient(format!("{id}: cannot reach {} ({kind})", self.base_url))
            }
        };
        let response = builder.send().await.map_err(|e| failed(e, false))?;
        let status = response.status().as_u16();
        let text = response.text().await.map_err(|e| failed(e, true))?;
        let fatal = |m: String| RequestError::Adapter(AdapterError::Fatal(m));
        match status {
            401 | 403 => return Err(fatal(format!("{} (HTTP {status}: {})", self.auth_hint, excerpt(&text)))),
            404 => return Err(fatal(format!("HTTP 404 from {}{path}: {}", self.base_url, excerpt(&text)))),
            429 => {
                return Err(RequestError::Adapter(AdapterError::rate_limited(format!(
                    "rate limited (HTTP 429): {}",
                    excerpt(&text)
                ))));
            }
            400 | 422 => return Err(RequestError::BadRequest(format!("HTTP {status}: {}", excerpt(&text)))),
            s if s >= 400 => {
                return Err(RequestError::Adapter(AdapterError::Transient {
                    message: format!("HTTP {status}: {}", excerpt(&text)),
                    rate_limited: matches!(s, 503 | 529),
                }));
            }
            _ => {}
        }
        serde_json::from_str(&text).map_err(|_| transient(format!("{id}: the server did not return JSON")))
    }
}

/// A context window that fits the prompt plus room for the answer. Ollama's default (4k tokens below 24 GB of
/// VRAM) is shorter than most Tellfold prompts and silently cuts the middle out of them.
pub fn ollama_context_length(text: &str) -> i64 {
    let needed = text.chars().count() as i64 / 3 + 4096; // about 3 characters per token, plus the reply
    let mut size = 8192;
    while size < needed && size < 65536 {
        size *= 2;
    }
    size
}

fn messages(request: &AdapterRequest) -> Value {
    json!([{"role": "system", "content": request.system}, {"role": "user", "content": request.prompt}])
}

fn model_names(data: &Value, key: &str, field: &str) -> Vec<String> {
    data.get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| m.get(field).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string))
        .collect()
}

/// `POST {base}/api/chat` with `stream: false` and `format` = the JSON schema for json() calls.
pub struct Ollama {
    pub http: HttpClient,
    default_model: Mutex<Option<String>>,
}

impl Ollama {
    pub fn new(base_url: &str) -> Ollama {
        Ollama { http: HttpClient::new("ollama", base_url, None), default_model: Mutex::new(None) }
    }

    pub async fn list_models(&self) -> Result<Vec<String>, RequestError> {
        let data = self.http.request(reqwest::Method::GET, "/api/tags", None, &[], Some(5.0)).await?;
        Ok(model_names(&data, "models", "name"))
    }

    async fn model(&self, request: &AdapterRequest) -> Result<String, AdapterError> {
        if let Some(model) = request.model_or_none() {
            return Ok(model.into());
        }
        if let Some(model) = self.default_model.lock().expect("not poisoned").clone() {
            return Ok(model);
        }
        let models = self.list_models().await?;
        let first = models.into_iter().next().ok_or_else(|| {
            AdapterError::Fatal("Ollama has no models: run `ollama pull llama3.2` (or any model) first".into())
        })?;
        *self.default_model.lock().expect("not poisoned") = Some(first.clone());
        Ok(first)
    }
}

#[async_trait]
impl Adapter for Ollama {
    fn runtime_id(&self) -> &str {
        &self.http.runtime_id
    }

    async fn send(&self, request: &AdapterRequest) -> Result<AdapterReply, AdapterError> {
        let model = self.model(request).await?;
        let mut body = Map::new();
        body.insert("model".into(), json!(model));
        body.insert("stream".into(), json!(false));
        body.insert("messages".into(), messages(request));
        if let Some(schema) = &request.json_schema {
            body.insert("format".into(), schema.clone());
        }
        let num_ctx = ollama_context_length(&format!("{}{}", request.system, request.prompt));
        body.insert("options".into(), json!({"num_ctx": num_ctx}));
        let data =
            match self.http.request(reqwest::Method::POST, "/api/chat", Some(&Value::Object(body)), &[], None).await {
                Ok(data) => data,
                Err(e) => {
                    let e = AdapterError::from(e);
                    if let AdapterError::Fatal(m) = &e {
                        if m.to_lowercase().contains("not found") {
                            let shown = py_repr(&json!(model));
                            return Err(AdapterError::Fatal(format!(
                                "Ollama model {shown} is not pulled: run `ollama pull {model}`"
                            )));
                        }
                    }
                    return Err(e);
                }
            };
        let Some(content) = data.pointer("/message/content").and_then(Value::as_str) else {
            return Err(AdapterError::bad_output(
                "Ollama returned no message content",
                py_prefix(&py_str(&data), 2000),
            ));
        };
        Ok(AdapterReply {
            text: content.into(),
            structured: None,
            model: data.get("model").and_then(Value::as_str).map(str::to_string),
            output_tokens: count(data.get("eval_count")),
        })
    }
}

const JSON_MODES: &[&str] = &["json_schema", "json_object", "prompt"];

/// `POST {base}/chat/completions`. For json(): `response_format` json_schema, falling back to json_object, then
/// to the prompt-only JSON contract, when the server rejects a mode (HTTP 400/422). The fallback sticks for the
/// life of this adapter.
pub struct OpenAiCompat {
    pub http: HttpClient,
    default_model: Mutex<Option<String>>,
    json_mode: AtomicUsize,
}

impl OpenAiCompat {
    pub fn new(base_url: &str, runtime_id: Option<&str>, api_key_env: Option<&str>) -> OpenAiCompat {
        OpenAiCompat {
            http: HttpClient::new(runtime_id.unwrap_or("openai-compatible"), base_url, api_key_env),
            default_model: Mutex::new(None),
            json_mode: AtomicUsize::new(0),
        }
    }

    pub fn with_default_model(self, model: Option<&str>) -> OpenAiCompat {
        *self.default_model.lock().expect("not poisoned") = model.filter(|m| !m.is_empty()).map(str::to_string);
        self
    }

    pub async fn list_models(&self) -> Result<Vec<String>, RequestError> {
        let data = self.http.request(reqwest::Method::GET, "/models", None, &self.http.bearer(), Some(5.0)).await?;
        Ok(model_names(&data, "data", "id"))
    }

    async fn model(&self, request: &AdapterRequest) -> Result<String, AdapterError> {
        if let Some(model) = request.model_or_none() {
            return Ok(model.into());
        }
        if let Some(model) = self.default_model.lock().expect("not poisoned").clone() {
            return Ok(model);
        }
        let first = self.list_models().await?.into_iter().next().ok_or_else(|| {
            AdapterError::Fatal(format!("{}: no model configured and the server lists none", self.http.runtime_id))
        })?;
        *self.default_model.lock().expect("not poisoned") = Some(first.clone());
        Ok(first)
    }

    fn response_format(&self, request: &AdapterRequest, mode: usize) -> Option<Value> {
        static NAME: OnceLock<Regex> = OnceLock::new();
        let schema = request.json_schema.as_ref()?;
        match JSON_MODES[mode.min(JSON_MODES.len() - 1)] {
            "json_schema" => {
                let cleaned = NAME
                    .get_or_init(|| Regex::new(r"[^A-Za-z0-9_-]+").expect("valid pattern"))
                    .replace_all(&request.schema_name, "_");
                let name = match py_prefix(&cleaned, 64) {
                    "" => "result",
                    name => name,
                };
                Some(json!({"type": "json_schema", "json_schema": {"name": name, "schema": schema, "strict": false}}))
            }
            "json_object" => Some(json!({"type": "json_object"})),
            _ => None,
        }
    }

    async fn chat(&self, request: &AdapterRequest) -> Result<AdapterReply, AdapterError> {
        let model = self.model(request).await?;
        loop {
            let mut body = Map::new();
            body.insert("model".into(), json!(model));
            body.insert("messages".into(), messages(request));
            let mode = self.json_mode.load(Ordering::SeqCst);
            if let Some(format) = self.response_format(request, mode) {
                body.insert("response_format".into(), format);
            }
            let path = "/chat/completions";
            match self
                .http
                .request(reqwest::Method::POST, path, Some(&Value::Object(body)), &self.http.bearer(), None)
                .await
            {
                Ok(data) => return reply(&data),
                Err(RequestError::BadRequest(m)) => {
                    if request.json_schema.is_some() && mode < JSON_MODES.len() - 1 {
                        // Only the first of several requests refused in this mode moves on; the others retry with
                        // whatever mode is current, so concurrent 400s never skip json_object.
                        let _ = self.json_mode.compare_exchange(mode, mode + 1, Ordering::SeqCst, Ordering::SeqCst);
                        continue;
                    }
                    return Err(AdapterError::Fatal(m));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

fn reply(data: &Value) -> Result<AdapterReply, AdapterError> {
    let no_choices = || AdapterError::bad_output("the server returned no choices", py_prefix(&py_str(data), 2000));
    let choice = data.get("choices").and_then(|c| c.get(0)).ok_or_else(no_choices)?;
    let message = choice.get("message").filter(|m| m.is_object()).ok_or_else(no_choices)?;
    let content = match message.get("content") {
        Some(Value::Array(parts)) => Some(
            parts
                .iter()
                .filter_map(|p| p.as_object().map(|o| o.get("text").and_then(Value::as_str).unwrap_or("").to_string()))
                .collect::<String>(),
        ),
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    };
    let Some(content) = content.filter(|c| !py_strip(c).is_empty()) else {
        let refusal = truthy(message.get("refusal")).map(|r| format!(" (refusal: {})", py_str(r))).unwrap_or_default();
        return Err(AdapterError::bad_output(format!("empty reply{refusal}"), ""));
    };
    Ok(AdapterReply {
        text: content,
        structured: None,
        model: data.get("model").and_then(Value::as_str).map(str::to_string),
        output_tokens: count(data.pointer("/usage/completion_tokens")),
    })
}

#[async_trait]
impl Adapter for OpenAiCompat {
    fn runtime_id(&self) -> &str {
        &self.http.runtime_id
    }

    async fn send(&self, request: &AdapterRequest) -> Result<AdapterReply, AdapterError> {
        self.chat(request).await
    }
}

/// The OpenAI API: Chat Completions with `OPENAI_API_KEY`.
pub struct OpenAiApi {
    pub inner: OpenAiCompat,
}

impl OpenAiApi {
    pub fn new(base_url: Option<&str>) -> OpenAiApi {
        let mut inner =
            OpenAiCompat::new(base_url.unwrap_or(OPENAI_BASE_URL), Some("openai-api"), Some("OPENAI_API_KEY"));
        inner.http.auth_hint = "OPENAI_API_KEY is missing or invalid.".into();
        OpenAiApi { inner }
    }
}

#[async_trait]
impl Adapter for OpenAiApi {
    fn runtime_id(&self) -> &str {
        "openai-api"
    }

    async fn send(&self, request: &AdapterRequest) -> Result<AdapterReply, AdapterError> {
        if self.inner.http.api_key().is_none() {
            return Err(AdapterError::Fatal("OPENAI_API_KEY is not set".into()));
        }
        self.inner.chat(request).await
    }
}

/// Messages API (`POST /v1/messages`) with `ANTHROPIC_API_KEY`. Model aliases haiku/sonnet/opus map to the current
/// model ids. No sampling parameters and no `thinking` field are sent (current models reject some of them); JSON
/// comes from the prompt contract plus validation and repair.
pub struct AnthropicApi {
    pub http: HttpClient,
}

impl AnthropicApi {
    pub fn new(base_url: Option<&str>) -> AnthropicApi {
        let mut http =
            HttpClient::new("anthropic-api", base_url.unwrap_or(ANTHROPIC_BASE_URL), Some("ANTHROPIC_API_KEY"));
        http.auth_hint = "ANTHROPIC_API_KEY is missing or invalid.".into();
        AnthropicApi { http }
    }

    pub fn headers(&self) -> Vec<(&'static str, String)> {
        let mut headers = vec![("anthropic-version", ANTHROPIC_VERSION.to_string())];
        if let Some(key) = self.http.api_key() {
            headers.push(("x-api-key", key));
        }
        headers
    }

    pub fn model_id(model: Option<&str>) -> String {
        match model {
            None | Some("") => ANTHROPIC_MODELS[0].into(),
            Some(m) => ANTHROPIC_ALIASES.iter().find(|(alias, _)| *alias == m).map_or(m, |(_, id)| id).into(),
        }
    }

    pub async fn list_models(&self) -> Result<Vec<String>, RequestError> {
        let data = self.http.request(reqwest::Method::GET, "/v1/models", None, &self.headers(), Some(5.0)).await?;
        Ok(model_names(&data, "data", "id"))
    }
}

#[async_trait]
impl Adapter for AnthropicApi {
    fn runtime_id(&self) -> &str {
        &self.http.runtime_id
    }

    async fn send(&self, request: &AdapterRequest) -> Result<AdapterReply, AdapterError> {
        if self.http.api_key().is_none() {
            return Err(AdapterError::Fatal("ANTHROPIC_API_KEY is not set".into()));
        }
        let body = json!({
            "model": AnthropicApi::model_id(request.model_or_none()),
            "max_tokens": ANTHROPIC_MAX_TOKENS,
            "system": request.system,
            "messages": [{"role": "user", "content": request.prompt}],
        });
        let data = self.http.request(reqwest::Method::POST, "/v1/messages", Some(&body), &self.headers(), None).await?;
        let Value::Object(obj) = &data else { return Err(AdapterError::bad_output("unexpected response shape", "")) };
        if obj.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
            return Err(AdapterError::bad_output("the model declined this request", ""));
        }
        let text: String = obj
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .map(|b| b.get("text").and_then(Value::as_str).unwrap_or(""))
            .collect();
        Ok(AdapterReply {
            text,
            structured: None,
            model: obj.get("model").and_then(Value::as_str).map(str::to_string),
            output_tokens: count(data.pointer("/usage/output_tokens")),
        })
    }
}

/// The httpx exception class Python names in the same failure: a timeout while connecting or while waiting for
/// the response, a refused or unresolvable host, or a server that hung up mid-exchange.
fn httpx_error_name(e: &reqwest::Error, reading: bool) -> &'static str {
    match (e.is_timeout(), e.is_connect()) {
        (true, true) => "ConnectTimeout",
        (true, false) => "ReadTimeout",
        (false, true) => "ConnectError",
        (false, false) if reading || e.is_body() || e.is_decode() => "ReadError",
        (false, false) => "RemoteProtocolError",
    }
}
