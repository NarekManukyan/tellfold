//! Port of the FakeLLM routing test in tests/test_llm.py (the conftest fixture).

use serde::Deserialize;
use serde_json::json;
use tellfold_llm::fake::{FakeLlm, FakeReply, Route, Routes};
use tellfold_llm::{json_with, Llm, LlmError, Schema};

#[derive(Debug, Deserialize)]
struct Pick {
    color: String,
    #[allow(dead_code)]
    n: i64,
}

fn schema() -> Schema {
    Schema::from_value::<Pick>(json!({"title": "Pick", "type": "object"}))
}

#[tokio::test]
async fn fake_llm_routes_by_purpose() {
    let llm = FakeLlm::routed(
        Routes::new()
            .route("engine", Route::list([json!({"color": "a", "n": 1}), json!({"color": "b", "n": 2})]))
            .route("chat.ask", Route::func(|prompt, _| prompt.to_uppercase()))
            .default_route(Route::fixed("fallback")),
    );
    let s = schema();
    for want in ["a", "b", "b"] {
        assert_eq!(json_with::<Pick>(&llm, "p", "s", "m", "engine.round", &s).await.unwrap().color, want);
    }
    assert_eq!(llm.text("hi", "s", "m", "chat.ask").await.unwrap(), "HI");
    assert_eq!(llm.text("hi", "s", "m", "report.write").await.unwrap(), "fallback");
    assert_eq!(llm.purposes()[3..], ["chat.ask", "report.write"]);
    assert_eq!(llm.calls.lock().unwrap()[0], ("engine.round".into(), "m".into(), "p".into()));
}

#[tokio::test]
async fn fake_errors_and_invalid_output() {
    let llm = FakeLlm::new(|purpose, _, _| -> FakeReply {
        match purpose {
            "boom" => LlmError::new("boom: failed").into(),
            "text-json" => r#"{"color": "c", "n": 3}"#.into(),
            "dict" => json!({"a": [1, true, null], "b": "it's"}).into(),
            _ => json!({"color": 5}).into(),
        }
    });
    let s = schema();
    assert_eq!(llm.text("p", "s", "m", "boom").await.unwrap_err().0, "boom: failed");
    assert_eq!(json_with::<Pick>(&llm, "p", "s", "m", "text-json", &s).await.unwrap().color, "c");
    let err = json_with::<Pick>(&llm, "p", "s", "m", "bad", &s).await.unwrap_err();
    assert!(err.0.starts_with("bad: invalid fake output: "), "{}", err.0);
    assert_eq!(llm.text("p", "s", "m", "dict").await.unwrap(), r#"{'a': [1, True, None], 'b': 'it\'s'}"#);
}

#[tokio::test]
#[should_panic(expected = "no route for purpose \"graph.extract\"")]
async fn unrouted_purposes_fail_the_test() {
    let llm = FakeLlm::routed(Routes::new().route("engine", Route::fixed("x")));
    let _ = llm.text("p", "s", "m", "graph.extract").await;
}
