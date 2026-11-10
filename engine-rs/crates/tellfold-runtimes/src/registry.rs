//! The runtime catalogue: what each runtime is, how to detect it, and how to build its LLM (port of
//! `tellfold/runtimes/registry.py`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tellfold_llm::adapter::{Adapter, AdapterLlm, TOOL_DEFAULT};
use tellfold_llm::extract::raw_decode;
use tellfold_llm::Llm;
use tellfold_models::text::py_strip;

use crate::claude::{ClaudeCode, DEFAULT_THINK_PURPOSES, PLANNER_THINK_PURPOSES};
use crate::cli::{self, unsafe_runtime_allowed, CliAdapter, CliTool, UNSAFE_OPT_IN_ENV};
use crate::detect::{binary_version, find_binary, run_quick, AUTH_TIMEOUT_S};
use crate::http::{self, AnthropicApi, OpenAiApi, OpenAiCompat, ANTHROPIC_VERSION};
use crate::settings::{home_dir, OpenAiEndpoint, Settings};

pub const HTTP_DETECT_TIMEOUT_S: f64 = 1.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Agent,
    Planner,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Agent => "agent",
            Role::Planner => "planner",
        }
    }
}

pub const ROLES: [Role; 2] = [Role::Agent, Role::Planner];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Auth {
    Ok,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Cli,
    Http,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuntimeInfo {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<String>,
    pub auth: Auth,
    pub auth_hint: Option<String>,
    pub models: Vec<String>,
    pub default_models: BTreeMap<String, String>,
    pub tested: bool,
    pub notes: Option<String>,
}

/// No runtime with that id (or an OpenAI-compatible endpoint that is not configured).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct UnknownRuntime(pub String);

/// Adapter options passed through to the LLM (`timeout_s`, `retries`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LlmOptions {
    pub timeout_s: Option<f64>,
    pub retries: Option<u32>,
}

fn env_any(names: &[&str]) -> bool {
    names.iter().any(|n| std::env::var(n).is_ok_and(|v| !py_strip(&v).is_empty()))
}

fn home_file(parts: &[&str]) -> bool {
    parts.iter().fold(home_dir(), |p, part| p.join(part)).is_file()
}

// ---------- CLI runtimes ----------

/// How a CLI runtime's login is checked.
#[derive(Debug, Clone, Copy)]
pub enum AuthCheck {
    Claude,
    Cursor,
    Codex,
    FileOrEnv {
        hint: &'static str,
        envs: &'static [&'static str],
        files: &'static [&'static [&'static str]],
        absent: Auth,
    },
}

impl AuthCheck {
    pub async fn check(&self, binary: &str, check: bool) -> (Auth, Option<String>) {
        match *self {
            AuthCheck::Claude => {
                let hint = Some("Run `claude` once and use /login.".to_string());
                if !check {
                    return (Auth::Unknown, hint);
                }
                let argv = [binary, "auth", "status"].map(String::from);
                let (_, out) = run_quick(&argv, AUTH_TIMEOUT_S).await;
                let data = out.find('{').and_then(|start| raw_decode(&out, start));
                match data {
                    Some(Value::Object(obj)) => {
                        let logged_in =
                            obj.get("loggedIn").is_some_and(|v| !matches!(v, Value::Null | Value::Bool(false)));
                        (if logged_in { Auth::Ok } else { Auth::Missing }, hint)
                    }
                    _ => (Auth::Unknown, hint),
                }
            }
            AuthCheck::Cursor => {
                let hint = Some("Run `cursor-agent login` (or set CURSOR_API_KEY).".to_string());
                if env_any(&["CURSOR_API_KEY"]) {
                    return (Auth::Ok, hint);
                }
                if !check {
                    return (Auth::Unknown, hint);
                }
                let argv = [binary, "status"].map(String::from);
                let (_, out) = run_quick(&argv, AUTH_TIMEOUT_S + 5.0).await;
                let lowered = out.to_lowercase();
                let auth = if lowered.contains("not logged in") {
                    Auth::Missing
                } else if lowered.contains("logged in") {
                    Auth::Ok
                } else {
                    Auth::Unknown
                };
                (auth, hint)
            }
            AuthCheck::Codex => {
                let hint = Some("Run `codex login`.".to_string());
                if !check {
                    return (Auth::Unknown, hint);
                }
                let argv = [binary, "login", "status"].map(String::from);
                let auth = match run_quick(&argv, AUTH_TIMEOUT_S).await.0 {
                    None => Auth::Unknown,
                    Some(0) => Auth::Ok,
                    Some(_) => Auth::Missing,
                };
                (auth, hint)
            }
            AuthCheck::FileOrEnv { hint, envs, files, absent } => {
                let found = env_any(envs) || files.iter().any(|f| home_file(f));
                (if found { Auth::Ok } else { absent }, Some(hint.to_string()))
            }
        }
    }
}

/// One agent CLI in the catalogue.
pub struct CliSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub binaries: &'static [&'static str],
    /// None: Claude Code ([`ClaudeCode`]).
    pub tool: Option<fn() -> Box<dyn CliTool>>,
    pub auth: AuthCheck,
    pub models: &'static [&'static str],
    pub default_models: (&'static str, &'static str),
    pub tested: bool,
    pub notes: &'static str,
    pub accepts_claude_aliases: bool,
}

const TOOL_DEFAULT_MODELS: &[&str] = &[TOOL_DEFAULT];
const TOOL_DEFAULTS: (&str, &str) = (TOOL_DEFAULT, TOOL_DEFAULT);

fn boxed<T: CliTool + 'static>(tool: T) -> Box<dyn CliTool> {
    Box::new(tool)
}

pub static CLI_SPECS: &[CliSpec] = &[
    CliSpec {
        id: "claude-code",
        name: "Claude Code",
        binaries: &["claude"],
        tool: None,
        auth: AuthCheck::Claude,
        models: &["haiku", "sonnet", "opus"],
        default_models: ("haiku", "sonnet"),
        tested: true,
        notes: "claude-agent-sdk with tools off, no MCP servers, no project settings, in an empty folder.",
        accepts_claude_aliases: true,
    },
    CliSpec {
        id: "cursor-agent",
        name: "Cursor Agent",
        binaries: &["cursor-agent", "agent"],
        tool: Some(|| boxed(cli::CursorAgent)),
        auth: AuthCheck::Cursor,
        models: &[TOOL_DEFAULT, "gpt-5", "sonnet-4", "sonnet-4-thinking"],
        default_models: TOOL_DEFAULTS,
        tested: false,
        notes: "cursor-agent -p --output-format json --mode ask --sandbox enabled --trust; a real JSON call and a \
                full run verified with cursor-agent 2026.11.05.",
        accepts_claude_aliases: false,
    },
    CliSpec {
        id: "codex",
        name: "Codex CLI",
        binaries: &["codex"],
        tool: Some(|| boxed(cli::Codex)),
        auth: AuthCheck::Codex,
        models: TOOL_DEFAULT_MODELS,
        default_models: TOOL_DEFAULTS,
        tested: false,
        notes: "codex exec --sandbox read-only --ephemeral, prompt on stdin; from the official CLI reference.",
        accepts_claude_aliases: false,
    },
    CliSpec {
        id: "gemini-cli",
        name: "Gemini CLI",
        binaries: &["gemini"],
        tool: Some(|| boxed(cli::Gemini)),
        auth: AuthCheck::FileOrEnv {
            hint: "Run `gemini` once to sign in, or set GEMINI_API_KEY.",
            envs: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
            files: &[&[".gemini", "oauth_creds.json"]],
            absent: Auth::Unknown,
        },
        models: &["auto", "flash", "pro", "flash-lite"],
        default_models: ("flash", "pro"),
        tested: false,
        notes: "gemini --prompt ... --output-format json --approval-mode default; from the official CLI reference.",
        accepts_claude_aliases: false,
    },
    CliSpec {
        id: "opencode",
        name: "OpenCode",
        binaries: &["opencode"],
        tool: Some(|| boxed(cli::OpenCode)),
        auth: AuthCheck::FileOrEnv {
            hint: "Run `opencode auth login`.",
            envs: &[],
            files: &[&[".local", "share", "opencode", "auth.json"]],
            absent: Auth::Unknown,
        },
        models: TOOL_DEFAULT_MODELS,
        default_models: TOOL_DEFAULTS,
        tested: false,
        notes: "opencode run --format json --dir <tmp>, with OPENCODE_CONFIG_CONTENT denying bash, edit and \
                webfetch; models are provider/model. JSON event shape is parsed defensively.",
        accepts_claude_aliases: false,
    },
    CliSpec {
        id: "qwen-code",
        name: "Qwen Code",
        binaries: &["qwen"],
        tool: Some(|| boxed(cli::Qwen)),
        auth: AuthCheck::FileOrEnv {
            hint: "Run `qwen` once to sign in.",
            envs: &["DASHSCOPE_API_KEY"],
            files: &[&[".qwen", "oauth_creds.json"]],
            absent: Auth::Unknown,
        },
        models: TOOL_DEFAULT_MODELS,
        default_models: TOOL_DEFAULTS,
        tested: false,
        notes: "qwen --prompt ... --output-format json; from the Qwen Code headless docs.",
        accepts_claude_aliases: false,
    },
    CliSpec {
        id: "copilot",
        name: "GitHub Copilot CLI",
        binaries: &["copilot"],
        tool: Some(|| boxed(cli::Copilot)),
        auth: AuthCheck::FileOrEnv {
            hint: "Run `copilot` and use /login, or set GH_TOKEN.",
            envs: &["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"],
            files: &[],
            absent: Auth::Unknown,
        },
        models: TOOL_DEFAULT_MODELS,
        default_models: TOOL_DEFAULTS,
        tested: false,
        notes: "copilot -p ... -s --no-ask-user --deny-tool shell --deny-tool write; from GitHub's programmatic \
                reference.",
        accepts_claude_aliases: false,
    },
    CliSpec {
        id: "grok",
        name: "Grok CLI",
        binaries: &["grok"],
        tool: Some(|| boxed(cli::Grok)),
        auth: AuthCheck::FileOrEnv {
            hint: "Set GROK_API_KEY.",
            envs: &["GROK_API_KEY"],
            files: &[&[".grok", "user-settings.json"]],
            absent: Auth::Missing,
        },
        models: TOOL_DEFAULT_MODELS,
        default_models: TOOL_DEFAULTS,
        tested: false,
        notes: "Community grok-cli (superagent-ai), not an official xAI tool: grok --prompt ... --directory <tmp>. \
                Off unless TELLFOLD_ALLOW_UNSAFE_RUNTIMES lists grok: it has no way to turn off its bash and edit \
                tools in headless mode.",
        accepts_claude_aliases: false,
    },
    CliSpec {
        id: "droid",
        name: "Factory Droid",
        binaries: &["droid"],
        tool: Some(|| boxed(cli::Droid)),
        auth: AuthCheck::FileOrEnv {
            hint: "Run `droid` once to sign in, or set FACTORY_API_KEY.",
            envs: &["FACTORY_API_KEY"],
            files: &[],
            absent: Auth::Unknown,
        },
        models: TOOL_DEFAULT_MODELS,
        default_models: TOOL_DEFAULTS,
        tested: false,
        notes: "droid exec -o json --cwd <tmp> (read-only without --auto); from the Factory CLI reference.",
        accepts_claude_aliases: false,
    },
];

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn default_map(agent: &str, planner: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("agent".to_string(), agent.to_string()), ("planner".to_string(), planner.to_string())])
}

fn with_options<A: Adapter>(llm: AdapterLlm<A>, options: LlmOptions) -> AdapterLlm<A> {
    let llm = match options.timeout_s {
        Some(t) => llm.with_timeout(Duration::from_secs_f64(t)),
        None => llm,
    };
    match options.retries {
        Some(r) => llm.with_retries(r),
        None => llm,
    }
}

pub struct CliRuntime {
    pub spec: &'static CliSpec,
}

impl CliRuntime {
    pub fn binary(&self) -> Option<String> {
        find_binary(self.spec.binaries)
    }

    pub async fn detect(&self, check_auth: bool) -> RuntimeInfo {
        let spec = self.spec;
        let path = self.binary();
        let mut info = RuntimeInfo {
            id: spec.id.into(),
            kind: Kind::Cli,
            name: spec.name.into(),
            installed: path.is_some(),
            version: None,
            path: path.clone(),
            auth: Auth::Unknown,
            auth_hint: None,
            models: strings(spec.models),
            default_models: default_map(spec.default_models.0, spec.default_models.1),
            tested: spec.tested,
            notes: (!spec.notes.is_empty()).then(|| spec.notes.to_string()),
        };
        let Some(path) = path else {
            info.auth_hint = Some(format!("Install {} (`{}` was not found).", spec.name, spec.binaries[0]));
            return info;
        };
        info.version = binary_version(&path, &["--version"]).await;
        (info.auth, info.auth_hint) = spec.auth.check(&path, check_auth).await;
        let unsafe_tool = spec.tool.is_some_and(|make| make().is_unsafe());
        if unsafe_tool && !unsafe_runtime_allowed(spec.id) {
            info.auth = Auth::Missing;
            info.auth_hint = Some(format!(
                "Disabled: {} cannot turn off its shell and file tools, so text in a seed document could make it \
                 run commands. Set {UNSAFE_OPT_IN_ENV}={} to use it anyway.",
                spec.name, spec.id
            ));
        }
        info
    }

    pub fn static_default(&self, role: Role) -> String {
        match role {
            Role::Agent => self.spec.default_models.0,
            Role::Planner => self.spec.default_models.1,
        }
        .into()
    }

    pub fn llm(&self, concurrency: usize, log_path: Option<PathBuf>, think: bool, options: LlmOptions) -> Arc<dyn Llm> {
        let path = self.binary();
        match self.spec.tool {
            None => {
                let mut llm = ClaudeCode::isolated(concurrency, log_path, path).with_think_purposes(if think {
                    PLANNER_THINK_PURPOSES
                } else {
                    DEFAULT_THINK_PURPOSES
                });
                if let Some(t) = options.timeout_s {
                    llm.timeout = Duration::from_secs_f64(t);
                }
                if let Some(r) = options.retries {
                    llm.retries = r;
                }
                Arc::new(llm)
            }
            Some(make) => {
                let binary = path.unwrap_or_else(|| self.spec.binaries[0].to_string());
                let adapter = CliAdapter::boxed(make(), &binary);
                Arc::new(with_options(AdapterLlm::new(adapter, concurrency, log_path), options))
            }
        }
    }
}

// ---------- HTTP runtimes ----------

/// `GET url`: (status, JSON body); (None, None) when the server cannot be reached.
pub async fn get_json(url: &str, headers: &[(&str, String)], timeout_s: f64) -> (Option<u16>, Option<Value>) {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs_f64(timeout_s))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    else {
        return (None, None);
    };
    let mut request = client.get(url);
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let Ok(response) = request.send().await else { return (None, None) };
    let status = response.status().as_u16();
    match response.text().await {
        Ok(text) => (Some(status), serde_json::from_str(&text).ok()),
        Err(_) => (None, None),
    }
}

fn settings_model(settings: &Settings, id: &str, role: Role) -> Option<String> {
    let (runtime, model) = match role {
        Role::Agent => (&settings.agent_runtime, &settings.agent_model),
        Role::Planner => (&settings.planner_runtime, &settings.planner_model),
    };
    (runtime == id && !model.is_empty()).then(|| model.clone())
}

fn role_defaults(settings: &Settings, id: &str, fallback: &str) -> BTreeMap<String, String> {
    ROLES
        .iter()
        .map(|&r| (r.as_str().to_string(), settings_model(settings, id, r).unwrap_or_else(|| fallback.to_string())))
        .collect()
}

pub struct OllamaRuntime {
    pub settings: Settings,
}

impl OllamaRuntime {
    pub const ID: &'static str = "ollama";

    pub fn base_url(&self) -> &str {
        &self.settings.ollama_base_url
    }

    pub async fn detect(&self) -> RuntimeInfo {
        let base = self.base_url();
        let (status, tags) = get_json(&format!("{base}/api/tags"), &[], HTTP_DETECT_TIMEOUT_S).await;
        let models: Vec<String> = tags
            .as_ref()
            .and_then(|t| t.get("models"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| m.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).map(str::to_string))
            .collect();
        let reachable = status == Some(200);
        let mut version = None;
        if reachable {
            let (_, ver) = get_json(&format!("{base}/api/version"), &[], HTTP_DETECT_TIMEOUT_S).await;
            version = ver.and_then(|v| v.get("version").and_then(Value::as_str).map(str::to_string));
        }
        let auth_hint = if reachable {
            None
        } else if find_binary(&["ollama"]).is_some() {
            Some("Start Ollama (open the app or run `ollama serve`).".to_string())
        } else {
            Some("Install Ollama from ollama.com, then pull a model.".to_string())
        };
        let default = models.first().cloned().unwrap_or_default();
        let mut notes = "POST /api/chat with format = the JSON schema.".to_string();
        if models.is_empty() && reachable {
            notes.push_str(" No models pulled yet: `ollama pull llama3.2`.");
        }
        RuntimeInfo {
            id: Self::ID.into(),
            kind: Kind::Http,
            name: "Ollama".into(),
            installed: reachable,
            version,
            path: Some(base.to_string()),
            auth: if reachable { Auth::Ok } else { Auth::Unknown },
            auth_hint,
            default_models: role_defaults(&self.settings, Self::ID, &default),
            models,
            tested: false,
            notes: Some(notes),
        }
    }

    pub fn static_default(&self, role: Role) -> String {
        settings_model(&self.settings, Self::ID, role).unwrap_or_default()
    }

    pub fn llm(&self, concurrency: usize, log_path: Option<PathBuf>, options: LlmOptions) -> Arc<dyn Llm> {
        let mut adapter = http::Ollama::new(self.base_url());
        if let Some(t) = options.timeout_s {
            adapter.http.timeout_s = t;
        }
        Arc::new(with_options(AdapterLlm::new(adapter, concurrency, log_path), options))
    }
}

pub struct OpenAiCompatRuntime {
    pub settings: Settings,
    pub endpoint: OpenAiEndpoint,
}

impl OpenAiCompatRuntime {
    pub fn id(&self) -> String {
        self.endpoint.runtime_id()
    }

    fn headers(&self) -> Vec<(&'static str, String)> {
        let key = std::env::var(self.endpoint.api_key_env()).unwrap_or_default();
        let key = py_strip(&key);
        if key.is_empty() {
            Vec::new()
        } else {
            vec![("Authorization", format!("Bearer {key}"))]
        }
    }

    pub async fn detect(&self) -> RuntimeInfo {
        let ep = &self.endpoint;
        let headers = self.headers();
        let has_key = !headers.is_empty();
        let (status, data) = get_json(&format!("{}/models", ep.base_url), &headers, HTTP_DETECT_TIMEOUT_S).await;
        let listed = data
            .as_ref()
            .and_then(|d| d.get("data"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string));
        let mut models: Vec<String> = Vec::new();
        for m in ep.models.iter().cloned().chain(listed) {
            if !models.contains(&m) {
                models.push(m);
            }
        }
        let auth = match status {
            Some(401 | 403) => Auth::Missing,
            Some(200) => Auth::Ok, // the server answered with our credentials (or needs none)
            _ if has_key => Auth::Ok,
            _ => Auth::Unknown,
        };
        let default = models.first().cloned().unwrap_or_default();
        RuntimeInfo {
            id: self.id(),
            kind: Kind::Http,
            name: ep.name.clone(),
            installed: status.is_some_and(|s| s < 500),
            version: None,
            path: Some(ep.base_url.clone()),
            auth,
            auth_hint: Some(format!(
                "If the server needs a key, set {} (the Mac app does this from the Keychain).",
                ep.api_key_env()
            )),
            default_models: role_defaults(&self.settings, &self.id(), &default),
            models,
            tested: false,
            notes: Some(
                "POST /chat/completions with response_format json_schema, falling back to json_object, then to \
                 prompt-only JSON."
                    .into(),
            ),
        }
    }

    pub fn static_default(&self, role: Role) -> String {
        settings_model(&self.settings, &self.id(), role)
            .unwrap_or_else(|| self.endpoint.models.first().cloned().unwrap_or_default())
    }

    pub fn llm(&self, concurrency: usize, log_path: Option<PathBuf>, options: LlmOptions) -> Arc<dyn Llm> {
        let id = self.id();
        let mut adapter = OpenAiCompat::new(&self.endpoint.base_url, Some(&id), Some(&self.endpoint.api_key_env()));
        if let Some(t) = options.timeout_s {
            adapter.http.timeout_s = t;
        }
        Arc::new(with_options(AdapterLlm::new(adapter, concurrency, log_path), options))
    }
}

/// The Anthropic and OpenAI APIs: billed per token, keyed by an environment variable.
pub struct KeyedRuntime {
    pub settings: Settings,
    pub id: &'static str,
    pub name: &'static str,
    pub key_env: &'static str,
    pub base_url: String,
    pub models_path: &'static str,
    pub static_models: &'static [&'static str],
    pub defaults: (&'static str, &'static str),
    pub notes: &'static str,
    pub accepts_claude_aliases: bool,
}

impl KeyedRuntime {
    pub fn anthropic(settings: Settings) -> KeyedRuntime {
        KeyedRuntime {
            settings,
            id: "anthropic-api",
            name: "Anthropic API",
            key_env: "ANTHROPIC_API_KEY",
            base_url: http::ANTHROPIC_BASE_URL.into(),
            models_path: "/v1/models",
            static_models: http::ANTHROPIC_MODELS,
            defaults: ("claude-haiku-5-5", "claude-sonnet-5-5"),
            notes: "Messages API, billed per token. haiku/sonnet/opus map to the current model ids.",
            accepts_claude_aliases: true,
        }
    }

    pub fn openai(settings: Settings) -> KeyedRuntime {
        KeyedRuntime {
            settings,
            id: "openai-api",
            name: "OpenAI API",
            key_env: "OPENAI_API_KEY",
            base_url: http::OPENAI_BASE_URL.into(),
            models_path: "/models",
            static_models: http::OPENAI_MODELS,
            defaults: ("gpt-5-mini", "gpt-5"),
            notes:
                "Chat Completions, billed per token. Default model names are suggestions; pick any your key can use.",
            accepts_claude_aliases: false,
        }
    }

    fn auth_headers(&self, key: &str) -> Vec<(&'static str, String)> {
        if self.id == "anthropic-api" {
            vec![("x-api-key", key.to_string()), ("anthropic-version", ANTHROPIC_VERSION.to_string())]
        } else {
            vec![("Authorization", format!("Bearer {key}"))]
        }
    }

    pub async fn detect(&self, check_auth: bool) -> RuntimeInfo {
        let key = std::env::var(self.key_env).unwrap_or_default();
        let key = py_strip(&key);
        let mut auth = if key.is_empty() { Auth::Missing } else { Auth::Unknown };
        if !key.is_empty() && check_auth {
            let url = format!("{}{}", self.base_url, self.models_path);
            match get_json(&url, &self.auth_headers(key), 5.0).await.0 {
                Some(401 | 403) => auth = Auth::Missing,
                Some(200) => auth = Auth::Ok,
                _ => {}
            }
        }
        RuntimeInfo {
            id: self.id.into(),
            kind: Kind::Http,
            name: self.name.into(),
            installed: !key.is_empty(),
            version: None,
            path: Some(self.base_url.clone()),
            auth,
            auth_hint: Some(format!("Set {} (the Mac app stores it in the Keychain).", self.key_env)),
            models: strings(self.static_models),
            default_models: ROLES.iter().map(|&r| (r.as_str().to_string(), self.static_default(r))).collect(),
            tested: false,
            notes: Some(self.notes.into()),
        }
    }

    pub fn static_default(&self, role: Role) -> String {
        settings_model(&self.settings, self.id, role).unwrap_or_else(|| {
            match role {
                Role::Agent => self.defaults.0,
                Role::Planner => self.defaults.1,
            }
            .into()
        })
    }

    pub fn llm(&self, concurrency: usize, log_path: Option<PathBuf>, options: LlmOptions) -> Arc<dyn Llm> {
        if self.id == "anthropic-api" {
            let mut adapter = AnthropicApi::new(Some(&self.base_url));
            if let Some(t) = options.timeout_s {
                adapter.http.timeout_s = t;
            }
            return Arc::new(with_options(AdapterLlm::new(adapter, concurrency, log_path), options));
        }
        let mut adapter = OpenAiApi::new(Some(&self.base_url));
        if let Some(t) = options.timeout_s {
            adapter.inner.http.timeout_s = t;
        }
        Arc::new(with_options(AdapterLlm::new(adapter, concurrency, log_path), options))
    }
}

/// Any runtime in the catalogue.
pub enum Runtime {
    Cli(CliRuntime),
    Ollama(OllamaRuntime),
    OpenAiCompat(OpenAiCompatRuntime),
    Keyed(KeyedRuntime),
}

impl Runtime {
    pub fn id(&self) -> String {
        match self {
            Runtime::Cli(r) => r.spec.id.into(),
            Runtime::Ollama(_) => OllamaRuntime::ID.into(),
            Runtime::OpenAiCompat(r) => r.id(),
            Runtime::Keyed(r) => r.id.into(),
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            Runtime::Cli(_) => Kind::Cli,
            _ => Kind::Http,
        }
    }

    pub fn accepts_claude_aliases(&self) -> bool {
        match self {
            Runtime::Cli(r) => r.spec.accepts_claude_aliases,
            Runtime::Keyed(r) => r.accepts_claude_aliases,
            _ => false,
        }
    }

    pub async fn detect(&self, check_auth: bool) -> RuntimeInfo {
        match self {
            Runtime::Cli(r) => r.detect(check_auth).await,
            Runtime::Ollama(r) => r.detect().await,
            Runtime::OpenAiCompat(r) => r.detect().await,
            Runtime::Keyed(r) => r.detect(check_auth).await,
        }
    }

    pub fn static_default(&self, role: Role) -> String {
        match self {
            Runtime::Cli(r) => r.static_default(role),
            Runtime::Ollama(r) => r.static_default(role),
            Runtime::OpenAiCompat(r) => r.static_default(role),
            Runtime::Keyed(r) => r.static_default(role),
        }
    }

    pub fn llm(&self, concurrency: usize, log_path: Option<PathBuf>, think: bool, options: LlmOptions) -> Arc<dyn Llm> {
        match self {
            Runtime::Cli(r) => r.llm(concurrency, log_path, think, options),
            Runtime::Ollama(r) => r.llm(concurrency, log_path, options),
            Runtime::OpenAiCompat(r) => r.llm(concurrency, log_path, options),
            Runtime::Keyed(r) => r.llm(concurrency, log_path, options),
        }
    }
}

pub fn all_runtimes(settings: &Settings) -> Vec<Runtime> {
    let mut runtimes: Vec<Runtime> = CLI_SPECS.iter().map(|spec| Runtime::Cli(CliRuntime { spec })).collect();
    runtimes.push(Runtime::Ollama(OllamaRuntime { settings: settings.clone() }));
    runtimes.extend(settings.openai_endpoints.iter().map(|endpoint| {
        Runtime::OpenAiCompat(OpenAiCompatRuntime { settings: settings.clone(), endpoint: endpoint.clone() })
    }));
    runtimes.push(Runtime::Keyed(KeyedRuntime::anthropic(settings.clone())));
    runtimes.push(Runtime::Keyed(KeyedRuntime::openai(settings.clone())));
    runtimes
}

pub fn known_ids(settings: &Settings) -> Vec<String> {
    all_runtimes(settings).iter().map(Runtime::id).collect()
}

/// `repr()` of an id in an error message.
fn quoted(id: &str) -> String {
    tellfold_models::json::py_repr(&Value::String(id.into()))
}

pub fn get_runtime(runtime_id: &str, settings: &Settings) -> Result<Runtime, UnknownRuntime> {
    if let Some(spec) = CLI_SPECS.iter().find(|s| s.id == runtime_id) {
        return Ok(Runtime::Cli(CliRuntime { spec }));
    }
    match runtime_id {
        OllamaRuntime::ID => return Ok(Runtime::Ollama(OllamaRuntime { settings: settings.clone() })),
        "anthropic-api" => return Ok(Runtime::Keyed(KeyedRuntime::anthropic(settings.clone()))),
        "openai-api" => return Ok(Runtime::Keyed(KeyedRuntime::openai(settings.clone()))),
        _ => {}
    }
    let (prefix, endpoint_id) = runtime_id.split_once(':').unwrap_or((runtime_id, ""));
    if prefix == "openai-compatible" && !endpoint_id.is_empty() {
        return match settings.endpoint(endpoint_id) {
            Some(endpoint) => Ok(Runtime::OpenAiCompat(OpenAiCompatRuntime {
                settings: settings.clone(),
                endpoint: endpoint.clone(),
            })),
            None => Err(UnknownRuntime(format!(
                "no OpenAI-compatible endpoint {} is configured in settings",
                quoted(endpoint_id)
            ))),
        };
    }
    Err(UnknownRuntime(format!("unknown runtime {}", quoted(runtime_id))))
}
