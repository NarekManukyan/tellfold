//! Parity with the Python engine: fixtures written by `engine-rs/golden/make.py`.

mod common;

use std::path::Path;

use common::{block_on, fixture};
use serde_json::{json, Value};
use tellfold_llm::adapter::{AdapterError, AdapterRequest};
use tellfold_runtimes::claude::{classify_api_failure, json_system, ClaudeCode, Failure, PLANNER_THINK_PURPOSES};
use tellfold_runtimes::cli::{self, parse_json_reply, CliTool, OPENCODE_LOCKDOWN_CONFIG};
use tellfold_runtimes::detect::{parse_version, strip_ansi};
use tellfold_runtimes::http::{excerpt, ollama_context_length, AnthropicApi};
use tellfold_runtimes::settings::{key_env_for, Settings};

fn tool(class: &str) -> Box<dyn CliTool> {
    match class {
        "CursorAgentLLM" => Box::new(cli::CursorAgent),
        "CodexLLM" => Box::new(cli::Codex),
        "GeminiLLM" => Box::new(cli::Gemini),
        "QwenLLM" => Box::new(cli::Qwen),
        "OpenCodeLLM" => Box::new(cli::OpenCode),
        "CopilotLLM" => Box::new(cli::Copilot),
        "GrokLLM" => Box::new(cli::Grok),
        "DroidLLM" => Box::new(cli::Droid),
        other => panic!("unknown class {other}"),
    }
}

#[test]
fn cli_argv_and_tool_properties_match_python() {
    let cases = fixture("runtimes/cli_argv.json");
    for case in cases.as_array().unwrap() {
        let tool = tool(case["class"].as_str().unwrap());
        let request = AdapterRequest {
            prompt: "the prompt".into(),
            system: "  be brief\n".into(),
            model: case["model"].as_str().unwrap().into(),
            purpose: "engine.round".into(),
            json_schema: None,
            schema_name: "result".into(),
        };
        let (argv, stdin) = block_on(tool.build("/opt/tools/tool", &request, Path::new("/W")));
        assert_eq!(json!(argv), case["argv"], "{case}");
        assert_eq!(json!(stdin), case["stdin"], "{case}");
        assert_eq!(tool.runtime_id(), case["runtime_id"]);
        let extra: serde_json::Map<String, Value> =
            tool.extra_env().into_iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
        assert_eq!(Value::Object(extra), case["extra_env"]);
        assert_eq!(json!(tool.passthrough_env()), case["passthrough"]);
        assert_eq!(json!(tool.json_output()), case["json_output"]);
        assert_eq!(json!(tool.is_unsafe()), case["unsafe"]);
        assert_eq!(json!(tool.auth_hint()), case["auth_hint"]);
    }
}

fn outcome(result: Result<tellfold_llm::adapter::AdapterReply, AdapterError>) -> Value {
    match result {
        Ok(reply) => json!({"text": reply.text, "model": reply.model}),
        Err(AdapterError::Transient { message, rate_limited }) => {
            json!({"error": "transient", "message": message, "rate_limited": rate_limited})
        }
        Err(AdapterError::Fatal(message)) => json!({"error": "fatal", "message": message}),
        Err(AdapterError::BadOutput { message, reply_text }) => {
            json!({"error": "bad_output", "message": message, "reply_text": reply_text})
        }
        Err(other) => panic!("unexpected {other:?}"),
    }
}

#[test]
fn parse_json_reply_matches_python() {
    for case in fixture("runtimes/parse_json_reply.json").as_array().unwrap() {
        let mut expected = case.clone();
        let stdout = expected.as_object_mut().unwrap().remove("stdout").unwrap();
        let got = outcome(parse_json_reply(stdout.as_str().unwrap(), Some("HINT")));
        assert_eq!(got, expected, "stdout {stdout}");
    }
}

#[test]
fn error_excerpts_match_python() {
    for case in fixture("runtimes/excerpt.json").as_array().unwrap() {
        assert_eq!(excerpt(case["body"].as_str().unwrap()), case["excerpt"], "{case}");
    }
}

#[test]
fn small_helpers_match_python() {
    let misc = fixture("runtimes/misc.json");
    for pair in misc["ollama_context"].as_array().unwrap() {
        let n = pair[0].as_u64().unwrap() as usize;
        assert_eq!(json!(ollama_context_length(&"x".repeat(n))), pair[1], "{n}");
    }
    for pair in misc["key_env_for"].as_array().unwrap() {
        assert_eq!(key_env_for(pair[0].as_str().unwrap()), pair[1]);
    }
    for pair in misc["parse_version"].as_array().unwrap() {
        assert_eq!(json!(parse_version(pair[0].as_str().unwrap())), pair[1], "{pair}");
    }
    for pair in misc["strip_ansi"].as_array().unwrap() {
        assert_eq!(strip_ansi(pair[0].as_str().unwrap()), pair[1]);
    }
    assert_eq!(OPENCODE_LOCKDOWN_CONFIG, misc["opencode_lockdown"]);
    for pair in misc["anthropic_model_id"].as_array().unwrap() {
        assert_eq!(AnthropicApi::model_id(pair[0].as_str()), pair[1]);
    }
}

#[test]
fn settings_validation_and_json_match_python() {
    for case in fixture("runtimes/settings.json").as_array().unwrap() {
        let parsed: Result<Settings, _> = serde_json::from_value(case["input"].clone());
        assert_eq!(parsed.is_ok(), case["ok"].as_bool().unwrap(), "{case} -> {parsed:?}");
        let Ok(settings) = parsed else { continue };
        assert_eq!(settings.to_json_pretty(), case["json"].as_str().unwrap());
        let endpoints: Vec<Value> = settings
            .openai_endpoints
            .iter()
            .map(|e| json!({"runtime_id": e.runtime_id(), "api_key_env": e.api_key_env()}))
            .collect();
        assert_eq!(json!(endpoints), case["endpoints"]);
    }
    let defaults = &fixture("runtimes/settings.json")[0];
    assert_eq!(Settings::default().to_json_pretty(), defaults["json"].as_str().unwrap());
}

#[test]
fn claude_code_command_line_matches_the_sdk() {
    for case in fixture("runtimes/claude_argv.json").as_array().unwrap() {
        let mut client = ClaudeCode::isolated(4, None, Some("/opt/bin/claude".into()));
        if case["think"] == json!(true) {
            client = client.with_think_purposes(PLANNER_THINK_PURPOSES);
        }
        let schema = Some(&case["schema"]).filter(|s| !s.is_null());
        let options =
            client.options(case["system"].as_str().unwrap(), "haiku", schema, case["purpose"].as_str().unwrap());
        let mut argv = options.argv("/opt/bin/claude");
        // added here: the SDK keeps sessions; `-p` replaces its initialize handshake
        assert_eq!(argv.split_off(argv.len() - 2), ["--no-session-persistence", "-p"]);
        assert_eq!(json!(argv), case["argv"], "{}", case["purpose"]);
    }
}

#[test]
fn claude_code_failure_classes_match_python() {
    for case in fixture("runtimes/claude_failures.json").as_array().unwrap() {
        let got = classify_api_failure(
            case["message"].as_str().unwrap(),
            case["status"].as_i64(),
            case["terminal_reason"].as_str(),
        );
        let (kind, text, rate_limited) = match got {
            Failure::Transient { message, rate_limited } => ("_Transient", message, json!(rate_limited)),
            Failure::BadOutput { message, .. } => ("_BadOutput", message, Value::Null),
            Failure::Llm(e) => ("LLMError", e.0, Value::Null),
        };
        assert_eq!(kind, case["kind"], "{case}");
        assert_eq!(text, case["text"], "{case}");
        assert_eq!(rate_limited, case["rate_limited"], "{case}");
    }
}

#[test]
fn claude_code_json_system_matches_python() {
    let system = fixture("runtimes/claude_system.json");
    assert_eq!(json_system("Sys \n", &system["schema"], true), system["structured"].as_str().unwrap());
    assert_eq!(json_system("Sys \n", &system["schema"], false), system["plain"].as_str().unwrap());
}
