//! Agent runtimes: settings, detection, CLI and HTTP adapters, routing (port of `tests/test_runtimes.py`).
//!
//! No test spawns a real agent CLI or calls a remote API: CLI adapters get a fake process runner, HTTP adapters a
//! local stub server. Detection runs against tiny shell scripts in a temp folder. Every test holds the
//! environment lock ([`Isolated`]), since several of them change environment variables.

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{block_on, closed_url, make_script, pick_schema, stub, FakeRunner, Isolated, Pick, Step, SECRET};
use serde_json::{json, Value};
use tellfold_llm::adapter::{AdapterLlm, AdapterRequest};
use tellfold_llm::{json_with, Llm, LlmError};
use tellfold_models::SimConfig;
use tellfold_runtimes::cli::{self, parse_json_reply, CliAdapter, CliTool, UNSAFE_OPT_IN_ENV};
use tellfold_runtimes::detect::{child_env, find_binary, parse_version};
use tellfold_runtimes::http::{self, AnthropicApi, OpenAiApi, OpenAiCompat, ANTHROPIC_VERSION};
use tellfold_runtimes::process::{self, ProcessResult};
use tellfold_runtimes::registry::{Auth, KeyedRuntime, Runtime};
use tellfold_runtimes::settings::{key_env_for, load_settings, save_settings, OpenAiEndpoint, Settings};
use tellfold_runtimes::{
    build_llm, detect_all, get_runtime, is_agent_purpose, known_ids, test_runtime, LlmOptions, Role, Route, RoutedLlm,
};

fn endpoint(id: &str, name: &str, base_url: &str, models: &[&str]) -> OpenAiEndpoint {
    OpenAiEndpoint::new(id, name, base_url, models.iter().map(|m| m.to_string()).collect(), None).unwrap()
}

fn text(llm: &dyn Llm, prompt: &str, model: &str) -> Result<String, LlmError> {
    block_on(llm.text(prompt, "s", model, "x"))
}

// ---------- settings ----------

#[test]
fn settings_defaults_roundtrip_and_location() {
    let iso = Isolated::new();
    assert_eq!(load_settings(None), Settings::default()); // missing file -> defaults
    let settings = Settings {
        agent_runtime: "ollama".into(),
        agent_model: "llama3.2:3b".into(),
        openai_endpoints: vec![endpoint("LMStudio", "LM Studio", "http://127.0.0.1:1234/v1/", &["qwen/qwen3-8b"])],
        ..Settings::default()
    };
    let path = save_settings(&settings, None).unwrap();
    assert_eq!(path, iso.path().join("tellfold-home").join("settings.json"));
    let loaded = load_settings(None);
    assert_eq!(loaded, settings);
    let ep = &loaded.openai_endpoints[0];
    assert_eq!(ep.id, "lmstudio");
    assert_eq!(ep.base_url, "http://127.0.0.1:1234/v1");
    assert_eq!(ep.runtime_id(), "openai-compatible:lmstudio");
    assert_eq!(ep.api_key_env(), "TELLFOLD_KEY_LMSTUDIO");
    std::fs::write(&path, "{not json").unwrap();
    assert_eq!(load_settings(None), Settings::default()); // corrupt file -> defaults, never fails
}

#[test]
fn settings_reject_secrets_and_bad_urls() {
    let _iso = Isolated::new();
    assert!(OpenAiEndpoint::new("x", "X", "http://h/v1", vec![], Some(&format!("{SECRET}!"))).is_err());
    assert!(OpenAiEndpoint::new("x", "X", "ftp://h/v1", vec![], None).is_err());
    assert!(OpenAiEndpoint::new("x", "X", "http://user:pw@h/v1", vec![], None).is_err());
    let twins = Settings {
        openai_endpoints: vec![endpoint("a", "A", "http://h", &[]), endpoint("a", "B", "http://h", &[])],
        ..Settings::default()
    };
    assert!(twins.validated().is_err());
    assert_eq!(key_env_for("open-router.ai"), "TELLFOLD_KEY_OPEN_ROUTER_AI");
}

#[test]
fn simconfig_runtime_fields_default_for_old_configs() {
    let old: SimConfig = serde_json::from_value(json!({"question": "q", "seed_files": []})).unwrap();
    assert_eq!(old.agent_runtime, "claude-code");
    assert_eq!(old.planner_runtime, "claude-code");
}

// ---------- detection ----------

#[test]
fn find_binary_searches_extra_dirs_and_child_env_puts_them_on_path() {
    let mut iso = Isolated::new();
    let local_bin = iso.home().join(".local").join("bin");
    let tool = make_script(&local_bin, "codex", r#"echo "codex-cli 0.9.1""#);
    iso.set("PATH", "/nonexistent");
    assert_eq!(find_binary(&["codex"]), Some(tool.to_string_lossy().into_owned()));
    let env = child_env(Some(&tool.to_string_lossy()), &[], &[]);
    assert_eq!(env["PATH"].split(':').next().unwrap(), local_bin.to_string_lossy());
    assert_eq!(parse_version("codex-cli 0.9.1\n").as_deref(), Some("0.9.1"));
    assert_eq!(parse_version("2026.01.23-916f423").as_deref(), Some("2026.01.23-916f423"));
}

#[test]
fn detect_cli_runtime_version_and_auth() {
    let iso = Isolated::new();
    make_script(
        &iso.bin(),
        "cursor-agent",
        r#"if [ "$1" = "--version" ]; then echo 2026.01.23-abc; elif [ "$1" = "status" ]; then echo " Not logged in"; fi"#,
    );
    make_script(
        &iso.bin(),
        "claude",
        r#"if [ "$1" = "--version" ]; then echo "2.1.283 (Claude Code)"; else echo '{"loggedIn": true, "email": "x@y"}'; fi"#,
    );
    let infos = block_on(detect_all(&Settings::default(), true));
    let by_id = |id: &str| infos.iter().find(|i| i.id == id).unwrap().clone();
    let cursor = by_id("cursor-agent");
    assert!(cursor.installed && cursor.version.as_deref() == Some("2026.01.23-abc") && cursor.auth == Auth::Missing);
    assert!(cursor.auth_hint.as_deref().unwrap().contains("cursor-agent login"));
    assert!(!cursor.tested);
    let claude = by_id("claude-code");
    assert!(
        claude.installed && claude.version.as_deref() == Some("2.1.283") && claude.auth == Auth::Ok && claude.tested
    );
    assert_eq!(json!(claude.default_models), json!({"agent": "haiku", "planner": "sonnet"}));
    assert!(!by_id("codex").installed && by_id("codex").path.is_none());
    // without check_auth no auth command runs
    let quick = block_on(detect_all(&Settings::default(), false));
    let quick_auth = |id: &str| quick.iter().find(|i| i.id == id).unwrap().auth;
    assert_eq!(quick_auth("cursor-agent"), Auth::Unknown);
    assert_eq!(quick_auth("claude-code"), Auth::Unknown);
}

#[test]
fn detect_all_covers_every_runtime_and_survives_unreachable_servers() {
    let _iso = Isolated::new();
    let settings = Settings {
        ollama_base_url: closed_url(),
        openai_endpoints: vec![endpoint("lmstudio", "LM Studio", &format!("{}/v1", closed_url()), &["m1"])],
        ..Settings::default()
    };
    let infos = block_on(detect_all(&settings, false));
    let ids: Vec<&str> = infos.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "claude-code",
            "cursor-agent",
            "codex",
            "gemini-cli",
            "opencode",
            "qwen-code",
            "copilot",
            "grok",
            "droid",
            "ollama",
            "openai-compatible:lmstudio",
            "anthropic-api",
            "openai-api"
        ]
    );
    assert_eq!(ids, known_ids(&settings));
    let lm = &infos[10];
    assert!(!lm.installed && lm.models == ["m1"] && lm.default_models["agent"] == "m1");
    assert!(infos.iter().all(|i| !i.installed));
    assert_eq!(infos[11].auth, Auth::Missing);
}

#[test]
fn detect_ollama_and_openai_compatible_with_stub_servers() {
    let mut iso = Isolated::new();
    block_on(async {
        let server = stub(|r| match r.path.as_str() {
            "/api/tags" => (200, json!({"models": [{"name": "llama3.2:3b"}, {"name": "qwen3:8b"}]}).to_string()),
            "/api/version" => (200, json!({"version": "0.12.0"}).to_string()),
            "/v1/models" if r.headers.get("authorization") != Some(&format!("Bearer {SECRET}")) => {
                (401, json!({"error": "no key"}).to_string())
            }
            "/v1/models" => (200, json!({"data": [{"id": "deepseek-chat"}]}).to_string()),
            _ => (404, String::new()),
        })
        .await;
        let settings = Settings {
            ollama_base_url: server.url.clone(),
            planner_runtime: "ollama".into(),
            planner_model: "qwen3:8b".into(),
            openai_endpoints: vec![endpoint("deepseek", "DeepSeek", &format!("{}/v1", server.url), &[])],
            ..Settings::default()
        };
        let infos = detect_all(&settings, false).await;
        let ollama = infos.iter().find(|i| i.id == "ollama").unwrap();
        assert!(ollama.installed && ollama.version.as_deref() == Some("0.12.0"));
        assert_eq!(ollama.models, ["llama3.2:3b", "qwen3:8b"]);
        assert_eq!(json!(ollama.default_models), json!({"agent": "llama3.2:3b", "planner": "qwen3:8b"}));
        let ds = infos.iter().find(|i| i.id == "openai-compatible:deepseek").unwrap();
        assert_eq!(ds.auth, Auth::Missing);
        iso.set("TELLFOLD_KEY_DEEPSEEK", SECRET);
        let ds = get_runtime("openai-compatible:deepseek", &settings).unwrap().detect(false).await;
        assert!(ds.installed && ds.auth == Auth::Ok && ds.models == ["deepseek-chat"]);
        assert!(!serde_json::to_string(&ds).unwrap().contains(SECRET));
    });
}

#[test]
fn keyed_api_runtimes_check_auth_only_when_asked() {
    let mut iso = Isolated::new();
    iso.set("ANTHROPIC_API_KEY", SECRET);
    block_on(async {
        let server = stub(|_| (200, json!({"data": [{"id": "claude-haiku-5-5"}]}).to_string())).await;
        let mut rt = KeyedRuntime::anthropic(Settings::default());
        rt.base_url = server.url.clone();
        let rt = Runtime::Keyed(rt);
        assert_eq!(rt.detect(false).await.auth, Auth::Unknown);
        assert!(server.requests.lock().unwrap().is_empty());
        let info = rt.detect(true).await;
        assert!(info.auth == Auth::Ok && info.installed);
        let first = server.requests.lock().unwrap()[0].clone();
        assert_eq!(first.headers["x-api-key"], SECRET);
        assert_eq!(first.headers["anthropic-version"], ANTHROPIC_VERSION);
    });
}

#[test]
fn get_runtime_unknown_ids() {
    let _iso = Isolated::new();
    assert_eq!(get_runtime("nope", &Settings::default()).err().unwrap().0, "unknown runtime 'nope'");
    assert!(get_runtime("openai-compatible:missing", &Settings::default()).is_err());
}

// ---------- CLI adapters (fake process runner) ----------

fn fake_bin(iso: &Isolated) -> String {
    let path = iso.path().join("tool");
    std::fs::write(&path, "").unwrap();
    path.to_string_lossy().into_owned()
}

fn cli_llm(tool: impl CliTool + 'static, binary: &str, runner: &Arc<FakeRunner>) -> AdapterLlm<CliAdapter> {
    AdapterLlm::new(CliAdapter::new(tool, binary).with_runner(runner.clone()), 4, None)
}

#[test]
fn cursor_agent_argv_isolation_and_json_parsing() {
    let iso = Isolated::new();
    let bin = fake_bin(&iso);
    let out = json!({"type": "result", "subtype": "success", "is_error": false, "result": "{\"color\":\"x\",\"n\":1}",
                     "session_id": "s"});
    let runner = FakeRunner::new(vec![ProcessResult::new(0, &out.to_string(), "").into()]);
    let llm = cli_llm(cli::CursorAgent, &bin, &runner);
    let pick: Pick =
        block_on(json_with(&llm, "the prompt", "be brief", "gpt-5", "engine.round", &pick_schema())).unwrap();
    assert_eq!(pick, Pick { color: "x".into(), n: 1 });
    let call = &runner.calls()[0];
    let argv = &call.argv;
    let after = |flag: &str| argv[argv.iter().position(|a| a == flag).unwrap() + 1].clone();
    assert_eq!(argv[..3], [bin.clone(), "-p".into(), "--output-format".into()]);
    assert_eq!(after("--mode"), "ask");
    assert_eq!(after("--sandbox"), "enabled");
    assert_eq!(after("--workspace"), call.cwd.to_string_lossy());
    assert!(call.cwd_was_empty);
    assert!(argv.contains(&"--trust".to_string())); // headless calls fail on the workspace-trust prompt without it
    assert_eq!(after("--model"), "gpt-5");
    for flag in ["--force", "--approve-mcps", "-f"] {
        assert!(!argv.contains(&flag.to_string()));
    }
    let last = argv.last().unwrap();
    assert!(last.contains("be brief") && last.contains("the prompt") && !last.starts_with('-'));
    assert!(!call.cwd.exists()); // the scratch folder is removed afterwards
}

#[test]
fn cursor_agent_tool_default_model_and_auth_failure() {
    let iso = Isolated::new();
    let bin = fake_bin(&iso);
    let runner = FakeRunner::new(vec![ProcessResult::new(
        1,
        "",
        "Error: Authentication required. Please run 'agent login' first.",
    )
    .into()]);
    let err = text(&cli_llm(cli::CursorAgent, &bin, &runner), "p", "default").unwrap_err();
    assert!(err.0.contains("cursor-agent login"), "{err}");
    let calls = runner.calls();
    assert_eq!(calls.len(), 1);
    assert!(!calls[0].argv.contains(&"--model".to_string()));
}

#[test]
fn cli_rate_limit_is_retried_and_crash_is_transient() {
    let iso = Isolated::new();
    let bin = fake_bin(&iso);
    let ok = json!({"type": "result", "is_error": false, "result": "fine"}).to_string();
    let runner = FakeRunner::new(vec![
        ProcessResult::new(1, "", "429 Too Many Requests").into(),
        ProcessResult::new(137, "", "").into(),
        ProcessResult::new(0, &ok, "").into(),
    ]);
    let llm = cli_llm(cli::CursorAgent, &bin, &runner).without_backoff();
    assert_eq!(text(&llm, "p", "").unwrap(), "fine");
    assert_eq!(runner.calls().len(), 3);
}

#[test]
fn cli_missing_binary_is_fatal() {
    let iso = Isolated::new();
    let missing = iso.path().join("nope").to_string_lossy().into_owned();
    let err = text(&cli_llm(cli::CursorAgent, &missing, &FakeRunner::new(vec![])), "p", "").unwrap_err();
    assert!(err.0.contains("not installed"), "{err}");
}

#[test]
fn codex_prompt_on_stdin_and_last_message_file() {
    let iso = Isolated::new();
    let bin = fake_bin(&iso);
    let runner = FakeRunner::new(vec![Step::Func(Box::new(|argv, _| {
        let at = argv.iter().position(|a| a == "--output-last-message").unwrap();
        std::fs::write(&argv[at + 1], "final answer\n").unwrap();
        ProcessResult::new(0, "lots of progress chatter", "")
    }))]);
    let llm = cli_llm(cli::Codex, &bin, &runner);
    assert_eq!(block_on(llm.text("question", "sys", "", "x")).unwrap(), "final answer");
    let call = &runner.calls()[0];
    let argv = &call.argv;
    assert_eq!(argv[1], "exec");
    assert_eq!(argv.last().unwrap(), "-");
    assert!(call.stdin.as_deref().unwrap().contains("question"));
    assert_eq!(argv[argv.iter().position(|a| a == "--sandbox").unwrap() + 1], "read-only");
    assert!(argv.contains(&"--skip-git-repo-check".to_string()));
    assert_eq!(argv[argv.iter().position(|a| a == "-C").unwrap() + 1], call.cwd.to_string_lossy());
    assert!(!argv.contains(&"-m".to_string()));
}

/// (tool, stdout, expected text, flags its argv must hold)
type AdapterCase = (Box<dyn CliTool>, String, &'static str, &'static [&'static str]);

#[test]
fn other_cli_adapters() {
    let mut iso = Isolated::new();
    iso.set(UNSAFE_OPT_IN_ENV, "grok"); // grok is off unless opted in
    let bin = fake_bin(&iso);
    let cases: Vec<AdapterCase> = vec![
        (
            Box::new(cli::Gemini),
            json!({"response": "gem", "stats": {}}).to_string(),
            "gem",
            &["--approval-mode", "default"],
        ),
        (
            Box::new(cli::Qwen),
            json!([{"type": "system"}, {"type": "result", "result": "qw"}]).to_string(),
            "qw",
            &["--prompt", "--approval-mode"],
        ),
        (
            Box::new(cli::OpenCode),
            "{\"type\":\"step_start\"}\n{\"type\":\"text\",\"part\":{\"type\":\"text\",\"text\":\"op\"}}\n\
             {\"type\":\"text\",\"part\":{\"type\":\"text\",\"text\":\"en\"}}\n"
                .into(),
            "open",
            &["run", "--format"],
        ),
        (
            Box::new(cli::Copilot),
            "plain copilot answer\n".into(),
            "plain copilot answer",
            &["--deny-tool", "--no-ask-user"],
        ),
        (Box::new(cli::Grok), "\x1b[1mgrok says\x1b[0m\n".into(), "grok says", &["--directory", "--prompt"]),
        (
            Box::new(cli::Droid),
            json!({"type": "result", "is_error": false, "result": "dr"}).to_string(),
            "dr",
            &["exec", "--cwd"],
        ),
    ];
    for (tool, stdout, expected, flags) in cases {
        let id = tool.runtime_id().to_string();
        let runner = FakeRunner::new(vec![ProcessResult::new(0, &stdout, "").into()]);
        let llm = AdapterLlm::new(CliAdapter::boxed(tool, &bin).with_runner(runner.clone()), 4, None);
        assert_eq!(text(&llm, "p", "m1").unwrap(), expected, "{id}");
        let argv = &runner.calls()[0].argv;
        for flag in flags {
            assert!(argv.contains(&flag.to_string()), "{id}: {flag}");
        }
        assert!(argv.contains(&"m1".to_string()), "{id}");
        for dangerous in ["--yolo", "--auto", "--allow-all-tools", "--force", "--skip-permissions-unsafe"] {
            assert!(!argv.contains(&dangerous.to_string()), "{id}: {dangerous}");
        }
    }
}

const ENGINE_SECRETS: &[(&str, &str)] = &[
    ("TELLFOLD_SERVE_TOKEN", "engine-token-0123456789"),
    ("TELLFOLD_KEY_DEEPSEEK", SECRET),
    ("ANTHROPIC_API_KEY", SECRET),
    ("OPENAI_API_KEY", SECRET),
];

fn all_cli_tools() -> Vec<Box<dyn CliTool>> {
    vec![
        Box::new(cli::CursorAgent),
        Box::new(cli::Codex),
        Box::new(cli::Gemini),
        Box::new(cli::Qwen),
        Box::new(cli::OpenCode),
        Box::new(cli::Copilot),
        Box::new(cli::Grok),
        Box::new(cli::Droid),
    ]
}

#[test]
fn cli_children_never_see_engine_secrets() {
    let mut iso = Isolated::new();
    for (name, value) in ENGINE_SECRETS {
        iso.set(name, value);
    }
    iso.set("SOME_UNRELATED_VAR", "x");
    iso.set("GEMINI_API_KEY", "gem-key");
    iso.set("LC_ALL", "en_US.UTF-8");
    iso.set(UNSAFE_OPT_IN_ENV, "grok");
    let bin = fake_bin(&iso);
    for tool in all_cli_tools() {
        let gemini = tool.runtime_id() == "gemini-cli";
        let out = json!({"result": "ok", "response": "ok"}).to_string();
        let runner = FakeRunner::new(vec![ProcessResult::new(0, &out, "").into()]);
        let llm = AdapterLlm::new(CliAdapter::boxed(tool, &bin).with_runner(runner.clone()), 4, None);
        text(&llm, "p", "").unwrap();
        let env = &runner.calls()[0].env;
        for (name, value) in ENGINE_SECRETS {
            assert!(!env.contains_key(*name), "{name}");
            assert!(!env.values().any(|v| v == value));
        }
        assert!(!env.contains_key("SOME_UNRELATED_VAR") && !env.contains_key(UNSAFE_OPT_IN_ENV));
        assert_eq!(env["HOME"], iso.home().to_string_lossy());
        assert_eq!(env["LC_ALL"], "en_US.UTF-8");
        assert!(!env["PATH"].is_empty());
        // each tool keeps only its own variables
        assert_eq!(env.contains_key("GEMINI_API_KEY"), gemini);
    }
}

#[test]
fn opencode_child_denies_bash_edit_and_webfetch() {
    let mut iso = Isolated::new();
    iso.set("OPENCODE_CONFIG_CONTENT", r#"{"permission": {"bash": "allow"}}"#); // must not leak through
    let bin = fake_bin(&iso);
    let out = "{\"type\":\"step_start\"}\n{\"type\":\"text\",\"part\":{\"type\":\"text\",\"text\":\"hi\"}}\n";
    let runner = FakeRunner::new(vec![ProcessResult::new(0, out, "").into()]);
    text(&cli_llm(cli::OpenCode, &bin, &runner), "p", "").unwrap();
    let config: Value = serde_json::from_str(&runner.calls()[0].env["OPENCODE_CONFIG_CONTENT"]).unwrap();
    assert_eq!(config["permission"], json!({"edit": "deny", "bash": "deny", "webfetch": "deny"}));
    assert_eq!(config["tools"]["bash"], json!(false));
    assert_eq!(config["tools"]["edit"], json!(false));
}

#[test]
fn gemini_trusts_only_the_empty_scratch_folder() {
    let iso = Isolated::new();
    let bin = fake_bin(&iso);
    let runner = FakeRunner::new(vec![ProcessResult::new(0, &json!({"response": "ok"}).to_string(), "").into()]);
    text(&cli_llm(cli::Gemini, &bin, &runner), "p", "").unwrap();
    let call = &runner.calls()[0];
    // folder trust is on by default and headless runs exit in an untrusted folder
    assert_eq!(call.env["GEMINI_CLI_TRUST_WORKSPACE"], "true");
    assert!(call.cwd_was_empty);
    assert!(!call.argv.contains(&"--yolo".to_string()));
    assert_eq!(call.argv[call.argv.iter().position(|a| a == "--approval-mode").unwrap() + 1], "default");
}

#[test]
fn opencode_v2_runs_its_own_server() {
    let iso = Isolated::new();
    for (i, (version, standalone)) in [("1.19.5", false), ("opencode 2.2.6", true), ("", false)].into_iter().enumerate()
    {
        let binary = make_script(&iso.path().join(format!("bin{i}")), "opencode", &format!(r#"echo "{version}""#));
        cli::clear_opencode_version_cache();
        let out = "{\"type\":\"step_start\"}\n{\"type\":\"text\",\"part\":{\"type\":\"text\",\"text\":\"hi\"}}\n";
        let runner = FakeRunner::new(vec![ProcessResult::new(0, out, "").into()]);
        text(&cli_llm(cli::OpenCode, &binary.to_string_lossy(), &runner), "p", "").unwrap();
        let argv = &runner.calls()[0].argv;
        assert_eq!(argv.contains(&"--standalone".to_string()), standalone, "{version}");
        assert!(!argv.contains(&"--dir".to_string())); // v2 removed it; the scratch folder is the working directory
    }
}

#[test]
fn ollama_context_fits_long_prompts() {
    assert_eq!(http::ollama_context_length("short"), 8192);
    assert_eq!(http::ollama_context_length(&"x".repeat(60_000)), 32768);
    assert_eq!(http::ollama_context_length(&"x".repeat(10_000_000)), 65536);
}

#[test]
fn grok_is_disabled_unless_opted_in() {
    let mut iso = Isolated::new();
    let bin = fake_bin(&iso);
    let runner = FakeRunner::new(vec![]);
    let err = text(&cli_llm(cli::Grok, &bin, &runner), "p", "").unwrap_err();
    assert!(err.0.contains(UNSAFE_OPT_IN_ENV), "{err}");
    assert!(runner.calls().is_empty()); // never spawned, never retried
    make_script(&iso.bin(), "grok", "echo 1.0.0");
    iso.set("GROK_API_KEY", "k");
    let info = block_on(get_runtime("grok", &Settings::default()).unwrap().detect(false));
    assert!(info.installed && info.auth == Auth::Missing);
    assert!(info.auth_hint.unwrap().contains(UNSAFE_OPT_IN_ENV));
    iso.set(UNSAFE_OPT_IN_ENV, "opencode, grok");
    assert_eq!(block_on(get_runtime("grok", &Settings::default()).unwrap().detect(false)).auth, Auth::Ok);
}

#[test]
fn claude_code_options_blank_engine_secrets() {
    let mut iso = Isolated::new();
    for (name, value) in ENGINE_SECRETS {
        iso.set(name, value);
    }
    let llm = tellfold_runtimes::claude::ClaudeCode::isolated(4, None, Some("/bin/true".into()));
    let options = llm.options("sys", "haiku", None, "x");
    for (name, _) in ENGINE_SECRETS {
        assert_eq!(options.env[*name], "", "{name}");
    }
    iso.remove("ANTHROPIC_API_KEY");
    // blanked even when unset, so a key from the user's shell profile never switches Claude Code to API billing
    assert_eq!(llm.options("s", "haiku", None, "x").env["ANTHROPIC_API_KEY"], "");
}

/// The environment with a PATH that has the system tools (the isolated PATH holds only the temp bin folder).
fn system_env() -> std::collections::BTreeMap<String, String> {
    let mut env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    env.insert("PATH".into(), "/usr/bin:/bin".into());
    env
}

fn sleep_script(iso: &Isolated) -> String {
    make_script(&iso.path(), "slow", "exec sleep 30").to_string_lossy().into_owned()
}

#[test]
fn live_process_groups_are_tracked_and_killed() {
    let iso = Isolated::new();
    let script = sleep_script(&iso);
    let cwd = iso.path();
    let (live, result) = block_on(async {
        let env = system_env();
        let argv = vec![script];
        let task = tokio::spawn(async move { process::run_process(&argv, None, &cwd, &env).await });
        for _ in 0..200 {
            if !process::live_process_groups().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let live = process::live_process_groups();
        process::kill_live_process_groups(); // what the force-exit path does
        (live, tokio::time::timeout(Duration::from_secs(10), task).await.unwrap().unwrap().unwrap())
    });
    assert_eq!(live.len(), 1);
    assert_eq!(result.returncode, -9);
    assert!(process::live_process_groups().is_empty());
}

#[test]
fn processes_a_tool_leaves_behind_are_ended() {
    let iso = Isolated::new();
    let pid_file = iso.path().join("worker.pid");
    let script = make_script(
        &iso.path().join("bin2"),
        "tool",
        &format!("(trap '' TERM; sleep 30) >/dev/null 2>&1 &\necho $! > {}\necho done", pid_file.display()),
    );
    let env = system_env();
    let argv = vec![script.to_string_lossy().into_owned()];
    let result = block_on(process::run_process(&argv, None, &iso.path(), &env)).unwrap();
    assert_eq!(result.returncode, 0);
    assert_eq!(result.stdout.trim(), "done");
    let worker: i32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
    // the signal lands asynchronously; the worker ignores SIGTERM, like Cursor's does
    for _ in 0..100 {
        // SAFETY: signal 0 only checks that the process exists.
        if unsafe { libc::kill(worker, 0) } != 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // SAFETY: cleaning up the test's own child.
    unsafe { libc::kill(worker, libc::SIGKILL) };
    panic!("the background worker outlived the call");
}

#[test]
fn parse_json_reply_errors() {
    use tellfold_llm::adapter::AdapterError;
    let fatal = parse_json_reply(&json!({"is_error": true, "result": "Please log in"}).to_string(), Some("log in!"));
    assert!(matches!(fatal, Err(AdapterError::Fatal(m)) if m.contains("log in")));
    let transient = parse_json_reply(&json!({"error": {"message": "quota exceeded"}}).to_string(), None);
    assert!(matches!(transient, Err(AdapterError::Transient { .. })));
    assert!(matches!(parse_json_reply("   ", None), Err(AdapterError::BadOutput { .. })));
    assert_eq!(parse_json_reply("not json at all", None).unwrap().text, "not json at all");
}

/// An adapter whose argv is just the binary and one argument.
struct Plain {
    stdin: Option<&'static str>,
}

#[async_trait::async_trait]
impl CliTool for Plain {
    fn runtime_id(&self) -> &str {
        "plain"
    }
    fn json_output(&self) -> bool {
        false
    }
    async fn build(&self, binary: &str, request: &AdapterRequest, _workdir: &Path) -> (Vec<String>, Option<String>) {
        (vec![binary.into(), request.prompt.clone()], self.stdin.map(str::to_string))
    }
}

#[test]
fn real_process_runner_kills_on_timeout() {
    let iso = Isolated::new();
    let script = sleep_script(&iso);
    let llm = AdapterLlm::new(CliAdapter::new(Plain { stdin: None }, &script), 4, None)
        .with_timeout(Duration::from_millis(500))
        .with_retries(0);
    let started = std::time::Instant::now();
    let err = text(&llm, "p", "").unwrap_err();
    assert!(err.0.contains("timed out"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(process::live_process_groups().is_empty());
}

#[test]
fn real_process_runner_passes_prompt_without_shell() {
    let iso = Isolated::new();
    let script = make_script(&iso.path(), "echo", r#"printf '%s|%s|%s' "$1" "$(pwd -P)" "$(cat)""#);
    let hostile = "$(touch pwned); `id`; rm -rf ~ && echo";
    let llm = AdapterLlm::new(CliAdapter::new(Plain { stdin: Some("from stdin") }, &script.to_string_lossy()), 4, None);
    let out = block_on(llm.text(hostile, "s", "", "x")).unwrap();
    let parts: Vec<&str> = out.split('|').collect();
    assert_eq!(parts[0], hostile);
    assert!(parts[1].contains("tellfold-rt-"), "{out}");
    assert_eq!(parts[2], "from stdin");
    assert!(!iso.path().join("pwned").exists());
}

// ---------- HTTP adapters (stub server) ----------

#[test]
fn ollama_chat_with_schema_and_auto_model() {
    let _iso = Isolated::new();
    block_on(async {
        let server = stub(|r| {
            if r.path == "/api/tags" {
                return (200, json!({"models": [{"name": "llama3.2:3b"}]}).to_string());
            }
            let body = r.json();
            let reply = json!({"model": body["model"], "eval_count": 7,
                               "message": {"role": "assistant", "content": "{\"color\": \"o\", \"n\": 4}"}});
            (200, reply.to_string())
        })
        .await;
        let llm = AdapterLlm::new(http::Ollama::new(&server.url), 4, None);
        let pick: Pick = json_with(&llm, "p", "s", "", "engine.round", &pick_schema()).await.unwrap();
        assert_eq!(pick, Pick { color: "o".into(), n: 4 });
        let requests = server.requests.lock().unwrap().clone();
        let body = requests.iter().find(|r| r.path == "/api/chat").unwrap().json();
        assert_eq!(body["model"], "llama3.2:3b");
        assert_eq!(body["stream"], json!(false));
        assert_eq!(body["format"]["properties"]["n"]["type"], "integer");
        let roles: Vec<&Value> = body["messages"].as_array().unwrap().iter().map(|m| &m["role"]).collect();
        assert_eq!(roles, [&json!("system"), &json!("user")]);
    });
}

#[test]
fn ollama_missing_model_and_unreachable() {
    let _iso = Isolated::new();
    block_on(async {
        let server = stub(|_| (404, json!({"error": "model 'zzz' not found, try pulling it first"}).to_string())).await;
        let llm = AdapterLlm::new(http::Ollama::new(&server.url), 4, None);
        let err = llm.text("p", "s", "zzz", "x").await.unwrap_err();
        assert!(err.0.contains("ollama pull zzz"), "{err}");
        let llm = AdapterLlm::new(http::Ollama::new(&closed_url()), 4, None).with_retries(1).without_backoff();
        let err = llm.text("p", "s", "m", "x").await.unwrap_err();
        assert!(err.0.contains("cannot reach") && err.0.ends_with("(ConnectError)"), "{err}");

        // a server that hangs up without answering: httpx's RemoteProtocolError
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                drop(socket);
            }
        });
        let llm = AdapterLlm::new(http::Ollama::new(&url), 4, None).with_retries(0);
        let err = llm.text("p", "s", "m", "x").await.unwrap_err();
        assert!(err.0.ends_with("(RemoteProtocolError)"), "{err}");
    });
}

#[test]
fn openai_compatible_falls_back_json_schema_then_object_then_prompt() {
    let mut iso = Isolated::new();
    iso.set("TELLFOLD_KEY_LOCAL", SECRET);
    let log = iso.path().join("calls.jsonl");
    block_on(async {
        let server = stub(|r| {
            assert_eq!(r.headers["authorization"], format!("Bearer {SECRET}"));
            if r.json().get("response_format").is_some() {
                return (400, json!({"error": {"message": "response_format not supported"}}).to_string());
            }
            let reply = json!({"model": "m", "usage": {"completion_tokens": 3},
                               "choices": [{"message": {"content": "{\"color\": \"c\", \"n\": 5}"}}]});
            (200, reply.to_string())
        })
        .await;
        let adapter = OpenAiCompat::new(
            &format!("{}/v1", server.url),
            Some("openai-compatible:local"),
            Some("TELLFOLD_KEY_LOCAL"),
        );
        let llm = AdapterLlm::new(adapter, 4, Some(log.clone()));
        let pick: Pick = json_with(&llm, "p", "s", "qwen/qwen3-8b", "x", &pick_schema()).await.unwrap();
        assert_eq!(pick.n, 5);
        let seen: Vec<Value> = server.requests.lock().unwrap().iter().map(|r| r.json()).collect();
        let modes: Vec<Value> = seen.iter().map(|b| b["response_format"]["type"].clone()).collect();
        assert_eq!(modes, [json!("json_schema"), json!("json_object"), Value::Null]);
        assert_eq!(seen[0]["response_format"]["json_schema"]["name"], "Pick");
        let again: Pick = json_with(&llm, "p", "s", "qwen/qwen3-8b", "x", &pick_schema()).await.unwrap();
        assert_eq!(again.n, 5);
        assert_eq!(server.requests.lock().unwrap().len(), 4); // the fallback sticks
    });
    assert!(!std::fs::read_to_string(&log).unwrap().contains(SECRET));
}

#[test]
fn openai_compatible_auth_failure_has_no_secret() {
    let mut iso = Isolated::new();
    iso.set("TELLFOLD_KEY_LOCAL", SECRET);
    block_on(async {
        let server = stub(|_| (401, json!({"error": {"message": "invalid key"}}).to_string())).await;
        let adapter = OpenAiCompat::new(&format!("{}/v1", server.url), None, Some("TELLFOLD_KEY_LOCAL"));
        let err = AdapterLlm::new(adapter, 4, None).text("p", "s", "m", "x").await.unwrap_err();
        assert!(err.0.contains("401") && !err.0.contains(SECRET), "{err}");
    });
}

#[test]
fn anthropic_api_headers_alias_mapping_and_refusal() {
    let mut iso = Isolated::new();
    iso.set("ANTHROPIC_API_KEY", SECRET);
    block_on(async {
        let replies = Arc::new(Mutex::new(vec![
            json!({"content": [{"type": "text", "text": "hello"}], "stop_reason": "end_turn", "usage": {"output_tokens": 2}}),
            json!({"content": [], "stop_reason": "refusal"}),
        ]));
        let server = stub(move |_| (200, replies.lock().unwrap().remove(0).to_string())).await;
        let llm = AdapterLlm::new(AnthropicApi::new(Some(&server.url)), 4, None).with_retries(0);
        assert_eq!(llm.text("p", "sys", "haiku", "x").await.unwrap(), "hello");
        let first = server.requests.lock().unwrap()[0].clone();
        let body = first.json();
        assert_eq!(first.path, "/v1/messages");
        assert_eq!(first.headers["x-api-key"], SECRET);
        assert_eq!(first.headers["anthropic-version"], ANTHROPIC_VERSION);
        assert_eq!(body["model"], "claude-haiku-5-5");
        assert_eq!(body["system"], "sys");
        assert!(body.get("temperature").is_none() && body.get("thinking").is_none());
        let err = llm.text("p", "sys", "sonnet", "x").await.unwrap_err();
        assert!(err.0.contains("declined"), "{err}");
    });
}

#[test]
fn keyed_apis_without_key_fail_fast() {
    let _iso = Isolated::new();
    block_on(async {
        let server = stub(|_| panic!("no request without a key")).await;
        let anthropic = AdapterLlm::new(AnthropicApi::new(Some(&server.url)), 4, None);
        let openai = AdapterLlm::new(OpenAiApi::new(Some(&server.url)), 4, None);
        for llm in [&anthropic as &dyn Llm, &openai] {
            let err = llm.text("p", "s", "m", "x").await.unwrap_err();
            assert!(err.0.contains("not set"), "{err}");
        }
        assert!(server.requests.lock().unwrap().is_empty());
    });
}

// ---------- routing and build_llm ----------

struct Recorder {
    name: &'static str,
    calls: Mutex<Vec<(String, String)>>,
}

#[async_trait::async_trait]
impl Llm for Recorder {
    async fn text(&self, _prompt: &str, _system: &str, model: &str, purpose: &str) -> Result<String, LlmError> {
        self.calls.lock().unwrap().push((purpose.into(), model.into()));
        Ok(self.name.into())
    }

    async fn json_value(
        &self,
        _prompt: &str,
        _system: &str,
        model: &str,
        purpose: &str,
        _schema: &tellfold_llm::Schema,
    ) -> Result<Value, LlmError> {
        self.calls.lock().unwrap().push((purpose.into(), model.into()));
        Ok(json!({"color": self.name, "n": 0}))
    }
}

#[test]
fn routed_llm_sends_agent_purposes_to_agent_runtime_and_maps_aliases() {
    let agent = Arc::new(Recorder { name: "agent", calls: Mutex::default() });
    let planner = Arc::new(Recorder { name: "planner", calls: Mutex::default() });
    let routed = RoutedLlm {
        agent: Route {
            llm: agent.clone(),
            runtime_id: "ollama".into(),
            default_model: "llama3.2".into(),
            accepts_claude_aliases: false,
        },
        planner: Route {
            llm: planner.clone(),
            runtime_id: "claude-code".into(),
            default_model: "sonnet".into(),
            accepts_claude_aliases: true,
        },
    };
    for purpose in ["engine.round", "engine.sentiment", "engine.seed", "chat.interview"] {
        assert!(is_agent_purpose(purpose));
        assert_eq!(block_on(routed.text("p", "s", "haiku", purpose)).unwrap(), "agent");
    }
    for purpose in ["graph.extract", "personas.batch", "personas.cast", "report.write", "chat.ask", "doctor.ping"] {
        assert!(!is_agent_purpose(purpose));
        let pick: Pick = block_on(json_with(&routed, "p", "s", "sonnet", purpose, &pick_schema())).unwrap();
        assert_eq!(pick.color, "planner");
    }
    assert!(agent.calls.lock().unwrap().iter().all(|(_, m)| m == "llama3.2")); // "haiku" means nothing to Ollama
    assert!(planner.calls.lock().unwrap().iter().all(|(_, m)| m == "sonnet"));
    block_on(routed.text("p", "s", "qwen3:8b", "engine.round")).unwrap();
    assert_eq!(agent.calls.lock().unwrap().last().unwrap(), &("engine.round".to_string(), "qwen3:8b".to_string()));
}

#[test]
fn build_llm_defaults_to_claude_code_and_shares_one_backend() {
    let iso = Isolated::new();
    let config = SimConfig { think: true, ..SimConfig::new("q", vec![]) };
    let llm =
        build_llm(&config, &Settings::default(), 3, Some(iso.path().join("calls.jsonl")), None, LlmOptions::default())
            .unwrap();
    assert!(Arc::ptr_eq(&llm.agent.llm, &llm.planner.llm));
    assert_eq!(llm.agent.runtime_id, "claude-code");
    // the Claude Code adapter itself: planner think purposes, an empty scratch folder, no setting sources
    let claude = tellfold_runtimes::claude::ClaudeCode::isolated(3, None, None)
        .with_think_purposes(tellfold_runtimes::claude::PLANNER_THINK_PURPOSES);
    assert_eq!(claude.think_purposes[0], "graph.");
    let options = claude.options("sys", "haiku", None, "engine.round");
    assert!(options.cwd.as_ref().unwrap().to_string_lossy().contains("tellfold-runtime-cwd"));
    assert!(options.argv("claude").contains(&"--setting-sources=".to_string()));
}

#[test]
fn build_llm_mixes_runtimes_and_uses_settings_models() {
    let _iso = Isolated::new();
    let settings = Settings {
        agent_runtime: "openai-compatible:lm".into(),
        agent_model: "qwen/qwen3-8b".into(),
        openai_endpoints: vec![endpoint("lm", "LM", "http://127.0.0.1:1234/v1", &["first"])],
        ..Settings::default()
    };
    let config = SimConfig {
        agent_runtime: "openai-compatible:lm".into(),
        planner_runtime: "anthropic-api".into(),
        ..SimConfig::new("q", vec![])
    };
    let llm = build_llm(&config, &settings, 2, None, None, LlmOptions::default()).unwrap();
    assert_eq!(llm.agent.runtime_id, "openai-compatible:lm");
    assert_eq!(llm.agent.default_model, "qwen/qwen3-8b");
    assert_eq!(llm.planner.runtime_id, "anthropic-api");
    assert!(llm.planner.accepts_claude_aliases);
    assert!(!Arc::ptr_eq(&llm.agent.llm, &llm.planner.llm));
    let mystery = SimConfig { planner_runtime: "mystery".into(), ..config };
    assert!(build_llm(&mystery, &settings, 1, None, None, LlmOptions::default()).is_err());
}

#[test]
fn test_runtime_reports_failure_without_raising() {
    let _iso = Isolated::new();
    let result = block_on(test_runtime("anthropic-api", &Settings::default(), None, Role::Agent, 120.0)).unwrap();
    assert_eq!(result["ok"], json!(false));
    assert!(result["error"].as_str().unwrap().contains("ANTHROPIC_API_KEY"));
    assert_eq!(result["model"], "claude-haiku-5-5");
    let keys: Vec<&String> = result.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["ok", "seconds", "reply", "error", "model"]);
    assert!(block_on(test_runtime("nope", &Settings::default(), None, Role::Agent, 120.0)).is_err());
}

#[test]
fn runtime_infos_serialize_like_python() {
    let _iso = Isolated::new();
    let info = block_on(get_runtime("codex", &Settings::default()).unwrap().detect(false));
    let value = serde_json::to_value(&info).unwrap();
    let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "id",
            "kind",
            "name",
            "installed",
            "version",
            "path",
            "auth",
            "auth_hint",
            "models",
            "default_models",
            "tested",
            "notes"
        ]
    );
    assert_eq!(value["kind"], "cli");
    assert_eq!(value["auth"], "unknown");
    assert_eq!(value["auth_hint"], "Install Codex CLI (`codex` was not found).");
}

#[test]
fn expand_user_matches_os_path_expanduser() {
    use tellfold_runtimes::settings::{expand_user, home_dir};
    let home = home_dir();
    assert_eq!(expand_user("~"), home);
    assert_eq!(expand_user("~/a/b"), home.join("a/b"));
    assert_eq!(expand_user("a/~/b"), std::path::PathBuf::from("a/~/b"));
    assert_eq!(expand_user("~no-such-user-tf/a"), std::path::PathBuf::from("~no-such-user-tf/a"));
    let root = expand_user("~root/x");
    assert!(root.is_absolute() && root.ends_with("x") && !root.starts_with("~"), "{root:?}");
}
