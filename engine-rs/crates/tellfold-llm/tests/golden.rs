//! Parity with the Python engine: prompts, JSON extraction, error classification and chunking compared with
//! fixtures written by `engine-rs/golden/make.py`.

use std::path::PathBuf;

use serde_json::{json, Value};
use tellfold_llm::adapter::{classify_failure, json_system, repair_prompt, AdapterError};
use tellfold_llm::ingest::{chunk_text, decode_text, load_documents, normalize};
use tellfold_llm::{embedded_schema, extract_json};
use tellfold_models::json::{py_dumps, py_dumps_ascii};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../golden/fixtures")
}

fn fixture(name: &str) -> Value {
    let path = fixtures().join(name);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap()
}

#[test]
fn json_system_prompts_match_python_for_every_schema() {
    for (name, want) in fixture("prompts/json_system.json").as_object().unwrap() {
        let schema: Value = serde_json::from_str(embedded_schema(name).unwrap()).unwrap();
        assert_eq!(json_system("You are a careful analyst.  \n", &schema), want.as_str().unwrap(), "{name}");
    }
}

#[test]
fn repair_prompts_match_python() {
    for case in fixture("prompts/repair.json").as_array().unwrap() {
        let s = |k: &str| case[k].as_str().unwrap();
        assert_eq!(repair_prompt(s("prompt"), s("problem"), s("previous")), s("out"));
    }
}

#[test]
fn failures_are_classified_like_python() {
    for case in fixture("prompts/classify.json").as_array().unwrap() {
        let got = classify_failure(case["message"].as_str().unwrap(), case["hint"].as_str());
        let (kind, text, rate_limited) = match &got {
            AdapterError::Transient { message, rate_limited } => ("Transient", message.clone(), *rate_limited),
            AdapterError::Fatal(message) => ("Fatal", message.clone(), false),
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(
            json!([kind, text, rate_limited]),
            json!([case["kind"], case["text"], case["rate_limited"]]),
            "{case}"
        );
    }
}

#[test]
fn ascii_dumps_match_python() {
    for case in fixture("prompts/ascii_dumps.json").as_array().unwrap() {
        let value = case["value"].as_str().unwrap();
        assert_eq!(py_dumps_ascii(value), case["dumps"].as_str().unwrap());
        assert_eq!(py_dumps_ascii(&json!([value, 1.5, null])), case["dumps_list"].as_str().unwrap());
    }
}

#[test]
fn extract_json_matches_python() {
    for case in fixture("extract_json.json").as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        let prefer = case["prefer"].as_str().unwrap().chars().next().unwrap();
        match extract_json(text, prefer) {
            Ok(value) => assert_eq!(Some(&value), case.get("value"), "{text:?} prefer {prefer}"),
            Err(e) => assert_eq!(Some(&json!(e.to_string())), case.get("error"), "{text:?} prefer {prefer}"),
        }
    }
}

#[test]
fn normalizing_and_chunking_match_python() {
    let corpus = fixture("ingest/corpus.json");
    let normalized = fixture("ingest/normalized.json");
    let mut docs = Vec::new();
    for (name, text) in corpus.as_object().unwrap() {
        let got = normalize(text.as_str().unwrap());
        assert_eq!(got, normalized[name].as_str().unwrap(), "{name}");
        if !got.is_empty() {
            docs.push((name.clone(), got));
        }
    }
    for (key, want) in fixture("ingest/chunks.json").as_object().unwrap() {
        let (size, overlap) = key.split_once('/').unwrap();
        let chunks = chunk_text(&docs, size.parse().unwrap(), overlap.parse().unwrap()).unwrap();
        let got: Value = serde_json::from_str(&py_dumps(&chunks)).unwrap();
        let want = want.as_array().unwrap();
        assert_eq!(got.as_array().unwrap().len(), want.len(), "{key}: chunk count");
        for (g, w) in got.as_array().unwrap().iter().zip(want) {
            assert_eq!(g, w, "{key}");
        }
    }
    for (key, want) in fixture("ingest/chunk_errors.json").as_object().unwrap() {
        let (size, overlap) = key.split_once('/').unwrap();
        let got = chunk_text(&[("a".into(), "b".into())], size.parse().unwrap(), overlap.parse().unwrap());
        assert_eq!(got.err().map(|e| e.to_string()).as_deref(), want.as_str(), "{key}");
    }
}

fn unhex(hex: &str) -> Vec<u8> {
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()
}

#[test]
fn text_encodings_decode_like_python() {
    for (name, case) in fixture("ingest/encodings.json").as_object().unwrap() {
        let raw = unhex(case["hex"].as_str().unwrap());
        assert_eq!(normalize(&decode_text(&raw)), case["text"].as_str().unwrap(), "{name}");
    }
}

#[test]
fn pdf_text_matches_pymupdf_up_to_whitespace() {
    let want = fixture("ingest/pdf.json");
    let docs = load_documents(&[fixtures().join("ingest/brief.pdf")]).unwrap();
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].0, want["name"].as_str().unwrap());
    let words = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(words(&docs[0].1), words(want["text"].as_str().unwrap()));
}

#[test]
fn every_embedded_schema_is_present() {
    for name in [
        "AgreementCall",
        "CampMatches",
        "CompareReport",
        "Extraction",
        "OpeningPosts",
        "PersonaBatch",
        "Report",
        "RewriteSuggestion",
        "RoundDecisions",
        "SentimentScores",
        "VoicePick",
    ] {
        let schema: Value = serde_json::from_str(embedded_schema(name).unwrap()).unwrap();
        assert_eq!(schema["title"], json!(name));
    }
}

#[test]
fn model_types_agree_with_their_schemas() {
    use tellfold_llm::JsonModel;
    use tellfold_models::Report;

    // the schema lists exactly the fields the Rust type writes, minus the ones the engine fills in itself
    let schema = Report::schema();
    let mut properties: Vec<&String> = schema.value["properties"].as_object().unwrap().keys().collect();
    properties.sort();
    let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../mac/Tellfold/Resources/Examples");
    for example in ["corner-cafe", "school-phones"] {
        let raw: Value =
            serde_json::from_str(&std::fs::read_to_string(examples.join(example).join("report.json")).unwrap())
                .unwrap();
        assert!(schema.validate(&raw).is_ok(), "{example}");
        let report: Report = serde_json::from_value(raw).unwrap();
        let written: Value = serde_json::from_str(&tellfold_models::json::to_string(&report)).unwrap();
        let mut fields: Vec<&String> = written.as_object().unwrap().keys().filter(|k| *k != "citation_audit").collect();
        fields.sort();
        assert_eq!(fields, properties);
    }
    assert_eq!(schema.name, "Report");
    assert_eq!(schema.root_opener(), '{');
}
