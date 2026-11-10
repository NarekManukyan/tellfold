//! User settings shared by the CLI, `tellfold serve` and the Mac app: `$TELLFOLD_HOME/settings.json` (port of
//! `tellfold/settings.py`).
//!
//! No secrets live here. API keys come only from environment variables (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`,
//! `TELLFOLD_KEY_<PROVIDER_ID>` or an endpoint's own `key_env`); the Mac app keeps them in the Keychain and passes
//! them to the engine process as environment variables.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tellfold_models::lax;
use tellfold_models::text::py_strip;

pub const HOME_ENV: &str = "TELLFOLD_HOME";
pub const SETTINGS_FILE: &str = "settings.json";
pub const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";
/// The app was called Shoal; its variables still work.
pub const LEGACY_ENV_PREFIX: &str = "SHOAL_";
pub const LEGACY_HOME_NAME: &str = "Shoal";

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid pattern"))
}

fn slug() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    regex(&R, r"^[a-z0-9][a-z0-9_-]{0,39}$")
}

fn env_name() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    regex(&R, r"^[A-Za-z_][A-Za-z0-9_]{0,99}$")
}

fn runtime_id_pattern() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    regex(&R, r"^[a-z0-9][a-z0-9._-]{0,39}(:[a-z0-9][a-z0-9_-]{0,39})?$")
}

fn model_pattern() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    regex(&R, r"^[A-Za-z0-9][A-Za-z0-9._:/@\[\]-]{0,119}$")
}

/// `os.environ[name]`, falling back to its old `SHOAL_` name for a `TELLFOLD_` variable.
pub fn env(name: &str) -> Option<String> {
    if let Ok(value) = std::env::var(name) {
        return Some(value);
    }
    let rest = name.strip_prefix("TELLFOLD_")?;
    std::env::var(format!("{LEGACY_ENV_PREFIX}{rest}")).ok()
}

/// The `SHOAL_` name a `TELLFOLD_` variable had before the rename.
pub fn legacy_env_name(name: &str) -> Option<String> {
    name.strip_prefix("TELLFOLD_").map(|rest| format!("{LEGACY_ENV_PREFIX}{rest}"))
}

/// The other spelling of an API-key variable across the rename: `SHOAL_KEY_X` <-> `TELLFOLD_KEY_X`.
pub fn renamed_key_env(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("SHOAL_KEY_") {
        return format!("TELLFOLD_KEY_{rest}");
    }
    if let Some(rest) = name.strip_prefix("TELLFOLD_KEY_") {
        return format!("SHOAL_KEY_{rest}");
    }
    name.to_string()
}

/// `Path.home()`.
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn expand_user(raw: &str) -> PathBuf {
    if raw == "~" {
        return home_dir();
    }
    match raw.strip_prefix("~/") {
        Some(rest) => home_dir().join(rest),
        None => PathBuf::from(raw),
    }
}

/// `$TELLFOLD_HOME`, else `~/Library/Application Support/Tellfold`.
///
/// The first time, an existing `.../Application Support/Shoal` folder is moved to `.../Tellfold`, the same
/// migration the Mac app does on launch.
pub fn tellfold_home() -> PathBuf {
    let raw = env(HOME_ENV).unwrap_or_default();
    let raw = py_strip(&raw);
    if !raw.is_empty() {
        return expand_user(raw);
    }
    let support = home_dir().join("Library").join("Application Support");
    let (home, legacy) = (support.join("Tellfold"), support.join(LEGACY_HOME_NAME));
    if !home.exists() && legacy.is_dir() && fs::rename(&legacy, &home).is_err() {
        return legacy;
    }
    home
}

pub fn default_settings_path() -> PathBuf {
    tellfold_home().join(SETTINGS_FILE)
}

/// The default environment variable that holds a provider's API key: `TELLFOLD_KEY_<PROVIDER_ID>`.
pub fn key_env_for(provider_id: &str) -> String {
    static R: OnceLock<Regex> = OnceLock::new();
    let upper = provider_id.to_uppercase();
    let replaced = regex(&R, r"[^A-Z0-9]+").replace_all(&upper, "_");
    format!("TELLFOLD_KEY_{}", replaced.trim_matches('_'))
}

fn check_url(value: &str) -> Result<String, String> {
    static R: OnceLock<Regex> = OnceLock::new();
    let value = py_strip(value).trim_end_matches('/');
    if !regex(&R, r"^https?://[^\s/]+").is_match(value) {
        return Err("must be an http:// or https:// URL".into());
    }
    let after = value.split_once("//").map_or("", |(_, rest)| rest);
    if after.split('/').next().unwrap_or("").contains('@') {
        return Err("must not contain credentials".into());
    }
    Ok(value.to_string())
}

fn strict_str(value: Option<&Value>, field: &str) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{field}: Input should be a valid string")),
    }
}

fn required_str(obj: &serde_json::Map<String, Value>, field: &str) -> Result<String, String> {
    strict_str(obj.get(field), field)?.ok_or_else(|| format!("{field}: Field required"))
}

fn optional_str(obj: &serde_json::Map<String, Value>, field: &str) -> Result<Option<String>, String> {
    match obj.get(field) {
        None | Some(Value::Null) => Ok(None),
        other => strict_str(other, field),
    }
}

/// One OpenAI-compatible server: LM Studio, llama.cpp, vLLM, OpenRouter, DeepSeek, ...
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Value")]
pub struct OpenAiEndpoint {
    /// slug, e.g. 'lmstudio'; the runtime id becomes 'openai-compatible:<id>'
    pub id: String,
    pub name: String,
    /// e.g. http://127.0.0.1:1234/v1 (the part before /chat/completions)
    pub base_url: String,
    pub models: Vec<String>,
    /// env var holding the API key; default TELLFOLD_KEY_<ID>. A name, never the key itself.
    pub key_env: Option<String>,
}

impl OpenAiEndpoint {
    /// `OpenAIEndpoint(...)`: validated and normalized, or the first problem.
    pub fn new(
        id: &str,
        name: &str,
        base_url: &str,
        models: Vec<String>,
        key_env: Option<&str>,
    ) -> Result<OpenAiEndpoint, String> {
        let id = py_strip(id).to_lowercase();
        if !slug().is_match(&id) {
            return Err("id: id must be a short slug: a-z, 0-9, '-' or '_'".into());
        }
        let name_len = name.chars().count();
        if name_len < 1 {
            return Err("name: String should have at least 1 character".into());
        }
        if name_len > 80 {
            return Err("name: String should have at most 80 characters".into());
        }
        let base_url = check_url(base_url).map_err(|e| format!("base_url: {e}"))?;
        let mut out: Vec<String> = Vec::new();
        for m in models.iter().map(|m| py_strip(m)).filter(|m| !m.is_empty()) {
            if !model_pattern().is_match(m) {
                return Err(format!("models: invalid model name '{m}'"));
            }
            if !out.iter().any(|seen| seen == m) {
                out.push(m.to_string());
            }
        }
        out.truncate(200);
        let key_env = match key_env.map(py_strip).filter(|v| !v.is_empty()) {
            None => None,
            Some(v) if env_name().is_match(v) => Some(v.to_string()),
            Some(_) => return Err("key_env: key_env must be an environment variable name, not a key".into()),
        };
        Ok(OpenAiEndpoint { id, name: name.to_string(), base_url, models: out, key_env })
    }

    pub fn runtime_id(&self) -> String {
        format!("openai-compatible:{}", self.id)
    }

    pub fn api_key_env(&self) -> String {
        self.key_env.clone().unwrap_or_else(|| key_env_for(&self.id))
    }
}

impl TryFrom<Value> for OpenAiEndpoint {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, String> {
        let Value::Object(obj) = value else { return Err("Input should be a valid dictionary".into()) };
        let models = match obj.get("models") {
            None => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|m| m.as_str().map(str::to_string).ok_or_else(|| "models: Input should be a valid string".into()))
                .collect::<Result<_, String>>()?,
            Some(_) => return Err("models: Input should be a valid list".into()),
        };
        OpenAiEndpoint::new(
            &required_str(&obj, "id")?,
            &required_str(&obj, "name")?,
            &required_str(&obj, "base_url")?,
            models,
            optional_str(&obj, "key_env")?.as_deref(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Value")]
pub struct Settings {
    /// None: the caller's default (the CLI's ./runs, the Mac app's own folder)
    pub runs_dir: Option<String>,
    pub agent_runtime: String,
    pub agent_model: String,
    pub planner_runtime: String,
    pub planner_model: String,
    pub ollama_base_url: String,
    pub openai_endpoints: Vec<OpenAiEndpoint>,
    pub concurrency: i64,
    pub think: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            runs_dir: None,
            agent_runtime: "claude-code".into(),
            agent_model: "haiku".into(),
            planner_runtime: "claude-code".into(),
            planner_model: "sonnet".into(),
            ollama_base_url: DEFAULT_OLLAMA_URL.into(),
            openai_endpoints: Vec::new(),
            concurrency: 4,
            think: false,
        }
    }
}

impl Settings {
    /// Runs the field validators on a settings value built in code, like `Settings(...)`.
    pub fn validated(self) -> Result<Settings, String> {
        let runtime = |field: &str, v: &str| -> Result<String, String> {
            let v = py_strip(v);
            if runtime_id_pattern().is_match(v) {
                Ok(v.to_string())
            } else {
                Err(format!("{field}: invalid runtime id"))
            }
        };
        let model = |field: &str, v: &str| -> Result<String, String> {
            let v = py_strip(v);
            if v.is_empty() || model_pattern().is_match(v) {
                Ok(v.to_string())
            } else {
                Err(format!("{field}: invalid model name"))
            }
        };
        if !(1..=16).contains(&self.concurrency) {
            return Err(if self.concurrency < 1 {
                "concurrency: Input should be greater than or equal to 1".into()
            } else {
                "concurrency: Input should be less than or equal to 16".into()
            });
        }
        let settings = Settings {
            agent_runtime: runtime("agent_runtime", &self.agent_runtime)?,
            agent_model: model("agent_model", &self.agent_model)?,
            planner_runtime: runtime("planner_runtime", &self.planner_runtime)?,
            planner_model: model("planner_model", &self.planner_model)?,
            ollama_base_url: check_url(&self.ollama_base_url).map_err(|e| format!("ollama_base_url: {e}"))?,
            ..self
        };
        let ids: Vec<&str> = settings.openai_endpoints.iter().map(|e| e.id.as_str()).collect();
        if ids.iter().enumerate().any(|(i, id)| ids[..i].contains(id)) {
            return Err("Value error, OpenAI-compatible endpoint ids must be unique".into());
        }
        Ok(settings)
    }

    pub fn endpoint(&self, endpoint_id: &str) -> Option<&OpenAiEndpoint> {
        self.openai_endpoints.iter().find(|e| e.id == endpoint_id)
    }

    /// `settings.model_dump_json(indent=2)`.
    pub fn to_json_pretty(&self) -> String {
        tellfold_models::json::to_string_pretty(self)
    }
}

impl TryFrom<Value> for Settings {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, String> {
        let Value::Object(obj) = value else { return Err("Input should be a valid dictionary".into()) };
        let d = Settings::default();
        let text = |field: &str, default: String| -> Result<String, String> {
            Ok(strict_str(obj.get(field), field)?.unwrap_or(default))
        };
        let concurrency = match obj.get("concurrency") {
            None => d.concurrency,
            Some(v) => lax::int(v.clone()).map_err(|e| format!("concurrency: {e}"))?,
        };
        let think = match obj.get("think") {
            None => d.think,
            Some(v) => lax::boolean(v.clone()).map_err(|e| format!("think: {e}"))?,
        };
        let openai_endpoints = match obj.get("openai_endpoints") {
            None => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| OpenAiEndpoint::try_from(item.clone()).map_err(|e| format!("openai_endpoints: {e}")))
                .collect::<Result<_, _>>()?,
            Some(_) => return Err("openai_endpoints: Input should be a valid list".into()),
        };
        Settings {
            runs_dir: optional_str(&obj, "runs_dir")?,
            agent_runtime: text("agent_runtime", d.agent_runtime)?,
            agent_model: text("agent_model", d.agent_model)?,
            planner_runtime: text("planner_runtime", d.planner_runtime)?,
            planner_model: text("planner_model", d.planner_model)?,
            ollama_base_url: text("ollama_base_url", d.ollama_base_url)?,
            openai_endpoints,
            concurrency,
            think,
        }
        .validated()
    }
}

/// Settings from disk; defaults when the file is missing or unreadable (never fails).
pub fn load_settings(path: Option<&Path>) -> Settings {
    let path = path.map(Path::to_path_buf).unwrap_or_else(default_settings_path);
    let Ok(text) = fs::read_to_string(&path) else { return Settings::default() };
    serde_json::from_str(&text).unwrap_or_default()
}

/// Write atomically (temp file + rename), creating the folder if needed.
pub fn save_settings(settings: &Settings, path: Option<&Path>) -> io::Result<PathBuf> {
    let path = path.map(Path::to_path_buf).unwrap_or_else(default_settings_path);
    let dir = path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no folder"))?;
    fs::create_dir_all(dir)?;
    let mut tmp = tempfile::Builder::new().prefix(".settings-").suffix(".json").tempfile_in(dir)?;
    tmp.write_all(format!("{}\n", settings.to_json_pretty()).as_bytes())?;
    tmp.persist(&path).map_err(|e| e.error)?;
    Ok(path)
}
