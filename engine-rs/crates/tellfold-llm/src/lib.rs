//! The model interface of the engine: the Rust port of `tellfold/llm.py` (the `LLM` protocol, `LLMError`,
//! `FakeLLM`, `extract_json`), `tellfold/runtimes/base.py` (`AdapterLLM`: retries, repair, call log) and
//! `tellfold/ingest.py` (seed documents and chunking).
//!
//! Every engine module asks a model through [`Llm`], with a short stable `purpose` (`"graph.extract"`,
//! `"engine.round"`, ...) so tests can route [`fake::FakeLlm`] answers by purpose. JSON answers are checked against
//! a [`Schema`]: the JSON Schema text pydantic generated (embedded from `engine-rs/schemas/`) plus the Rust type
//! that validates the reply.

pub mod adapter;
pub mod canned;
pub mod extract;
pub mod fake;
pub mod ingest;

use std::fmt;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::Value;

pub use extract::{extract_json, NoJson};

/// A call failed after all retries or returned unusable output. The text is the message the user sees.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct LlmError(pub String);

impl LlmError {
    pub fn new(message: impl Into<String>) -> Self {
        LlmError(message.into())
    }
}

/// The JSON Schema a reply must match, and the Rust type that checks it.
pub struct Schema {
    /// The schema's `title` (the pydantic class name), used as the structured-output name.
    pub name: String,
    pub value: Value,
    validate: fn(&Value) -> Result<(), String>,
}

impl fmt::Debug for Schema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Schema").field("name", &self.name).finish_non_exhaustive()
    }
}

fn validate_as<T: DeserializeOwned>(value: &Value) -> Result<(), String> {
    T::deserialize(value).map(|_| ()).map_err(|e| e.to_string())
}

impl Schema {
    /// `text` is the schema as pydantic's `model_json_schema()` wrote it; `T` validates replies against it.
    pub fn new<T: DeserializeOwned>(text: &str) -> Schema {
        let value: Value = serde_json::from_str(text).expect("embedded schemas are valid JSON");
        let name = value.get("title").and_then(Value::as_str).unwrap_or("result").to_string();
        Schema { name, value, validate: validate_as::<T> }
    }

    /// A schema built in code (tests, ad-hoc calls).
    pub fn from_value<T: DeserializeOwned>(value: Value) -> Schema {
        let name = value.get("title").and_then(Value::as_str).unwrap_or("result").to_string();
        Schema { name, value, validate: validate_as::<T> }
    }

    /// `pydantic.model_validate(value)`: Ok when the value is a valid instance, else the problem.
    pub fn validate(&self, value: &Value) -> Result<(), String> {
        (self.validate)(value)
    }

    /// `'['` when the schema's root is an array, else `'{'`.
    pub fn root_opener(&self) -> char {
        adapter::root_opener(&self.value)
    }
}

/// A type the engine asks models for. `schema()` is its pydantic schema.
pub trait JsonModel: DeserializeOwned + Send {
    fn schema() -> &'static Schema;
}

/// Implements [`JsonModel`] for a type, embedding `engine-rs/schemas/<Name>.json`.
#[macro_export]
macro_rules! json_model {
    ($ty:ty, $path:expr) => {
        impl $crate::JsonModel for $ty {
            fn schema() -> &'static $crate::Schema {
                static SCHEMA: ::std::sync::OnceLock<$crate::Schema> = ::std::sync::OnceLock::new();
                SCHEMA.get_or_init(|| $crate::Schema::new::<$ty>(include_str!($path)))
            }
        }
    };
}

json_model!(tellfold_models::Report, "../../../schemas/Report.json");
json_model!(tellfold_models::CompareReport, "../../../schemas/CompareReport.json");
json_model!(tellfold_models::RewriteSuggestion, "../../../schemas/RewriteSuggestion.json");
json_model!(tellfold_models::CampMatches, "../../../schemas/CampMatches.json");
json_model!(tellfold_models::AgreementCall, "../../../schemas/AgreementCall.json");

/// The schemas pydantic generated for the engine's own reply types, by class name. Types defined in later crates
/// (graph extraction, personas, round decisions) embed theirs from the same folder.
pub fn embedded_schema(name: &str) -> Option<&'static str> {
    static FILES: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
    FILES
        .get_or_init(|| {
            vec![
                ("AgreementCall", include_str!("../../../schemas/AgreementCall.json")),
                ("CampMatches", include_str!("../../../schemas/CampMatches.json")),
                ("CompareReport", include_str!("../../../schemas/CompareReport.json")),
                ("Extraction", include_str!("../../../schemas/Extraction.json")),
                ("OpeningPosts", include_str!("../../../schemas/OpeningPosts.json")),
                ("PersonaBatch", include_str!("../../../schemas/PersonaBatch.json")),
                ("Report", include_str!("../../../schemas/Report.json")),
                ("RewriteSuggestion", include_str!("../../../schemas/RewriteSuggestion.json")),
                ("RoundDecisions", include_str!("../../../schemas/RoundDecisions.json")),
                ("SentimentScores", include_str!("../../../schemas/SentimentScores.json")),
                ("VoicePick", include_str!("../../../schemas/VoicePick.json")),
            ]
        })
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, text)| *text)
}

/// The `LLM` protocol: one text or JSON answer per call.
#[async_trait]
pub trait Llm: Send + Sync {
    async fn text(&self, prompt: &str, system: &str, model: &str, purpose: &str) -> Result<String, LlmError>;

    /// A JSON value that `schema` validates.
    async fn json_value(
        &self,
        prompt: &str,
        system: &str,
        model: &str,
        purpose: &str,
        schema: &Schema,
    ) -> Result<Value, LlmError>;
}

/// `llm.json(..., schema=T)`: the reply as a `T`.
pub async fn json<T: JsonModel>(
    llm: &dyn Llm,
    prompt: &str,
    system: &str,
    model: &str,
    purpose: &str,
) -> Result<T, LlmError> {
    json_with::<T>(llm, prompt, system, model, purpose, T::schema()).await
}

/// Like [`json`] with an explicit schema (one built in code, or a type without [`JsonModel`]).
pub async fn json_with<T: DeserializeOwned>(
    llm: &dyn Llm,
    prompt: &str,
    system: &str,
    model: &str,
    purpose: &str,
    schema: &Schema,
) -> Result<T, LlmError> {
    let value = llm.json_value(prompt, system, model, purpose, schema).await?;
    serde_json::from_value(value).map_err(|e| LlmError(format!("{purpose}: invalid JSON after validation: {e}")))
}
