//! `FakeLlm`: the deterministic test double (port of `FakeLLM` in `tellfold/llm.py` and the routing helper in
//! `tests/conftest.py`). No test ever starts a real AI tool.
//!
//! The responder is called as `responder(purpose, prompt, schema)` and returns a [`FakeReply`]: text for
//! `text()`, a JSON value or JSON text for `json()`, or an error to make the call fail. Every call is recorded in
//! [`FakeLlm::calls`] as (purpose, model, prompt).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::future::BoxFuture;
use futures::FutureExt;
use serde_json::Value;

use crate::{Llm, LlmError, Schema};

#[derive(Debug, Clone, PartialEq)]
pub enum FakeReply {
    Text(String),
    Json(Value),
    Error(LlmError),
}

impl From<&str> for FakeReply {
    fn from(s: &str) -> Self {
        FakeReply::Text(s.into())
    }
}

impl From<String> for FakeReply {
    fn from(s: String) -> Self {
        FakeReply::Text(s)
    }
}

impl From<Value> for FakeReply {
    fn from(v: Value) -> Self {
        FakeReply::Json(v)
    }
}

impl From<LlmError> for FakeReply {
    fn from(e: LlmError) -> Self {
        FakeReply::Error(e)
    }
}

type Responder = dyn Fn(&str, &str, Option<&Schema>) -> BoxFuture<'static, FakeReply> + Send + Sync;

pub struct FakeLlm {
    responder: Arc<Responder>,
    /// (purpose, model, prompt) of every call, in order.
    pub calls: Mutex<Vec<(String, String, String)>>,
    /// (purpose, system prompt) of every call, in the same order.
    pub systems: Mutex<Vec<(String, String)>>,
}

impl FakeLlm {
    /// A fake answering through a plain function.
    pub fn new<F, R>(responder: F) -> Self
    where
        F: Fn(&str, &str, Option<&Schema>) -> R + Send + Sync + 'static,
        R: Into<FakeReply>,
    {
        let responder = Arc::new(responder);
        FakeLlm {
            responder: Arc::new(move |purpose, prompt, schema| {
                let reply = responder(purpose, prompt, schema).into();
                async move { reply }.boxed()
            }),
            calls: Mutex::new(Vec::new()),
            systems: Mutex::new(Vec::new()),
        }
    }

    /// A fake whose answers take time (concurrency and stop tests).
    pub fn new_async<F>(responder: F) -> Self
    where
        F: Fn(&str, &str, Option<&Schema>) -> BoxFuture<'static, FakeReply> + Send + Sync + 'static,
    {
        FakeLlm { responder: Arc::new(responder), calls: Mutex::new(Vec::new()), systems: Mutex::new(Vec::new()) }
    }

    /// A fake answering through [`Routes`].
    pub fn routed(routes: Routes) -> Self {
        let routes = Arc::new(routes);
        FakeLlm::new(move |purpose, prompt, schema| routes.respond(purpose, prompt, schema))
    }

    pub fn purposes(&self) -> Vec<String> {
        self.calls.lock().expect("not poisoned").iter().map(|c| c.0.clone()).collect()
    }

    fn record(&self, purpose: &str, model: &str, prompt: &str, system: &str) {
        self.calls.lock().expect("not poisoned").push((purpose.into(), model.into(), prompt.into()));
        self.systems.lock().expect("not poisoned").push((purpose.into(), system.into()));
    }
}

#[async_trait]
impl Llm for FakeLlm {
    async fn text(&self, prompt: &str, system: &str, model: &str, purpose: &str) -> Result<String, LlmError> {
        self.record(purpose, model, prompt, system);
        match (self.responder)(purpose, prompt, None).await {
            FakeReply::Text(text) => Ok(text),
            FakeReply::Json(value) => Ok(tellfold_models::json::py_str(&value)),
            FakeReply::Error(e) => Err(e),
        }
    }

    async fn json_value(
        &self,
        prompt: &str,
        system: &str,
        model: &str,
        purpose: &str,
        schema: &Schema,
    ) -> Result<Value, LlmError> {
        self.record(purpose, model, prompt, system);
        let value = match (self.responder)(purpose, prompt, Some(schema)).await {
            FakeReply::Error(e) => return Err(e),
            FakeReply::Json(value) => value,
            FakeReply::Text(text) => {
                serde_json::from_str(&text).map_err(|e| LlmError(format!("{purpose}: invalid fake output: {e}")))?
            }
        };
        schema.validate(&value).map_err(|e| LlmError(format!("{purpose}: invalid fake output: {e}")))?;
        Ok(value)
    }
}

/// A route's answer as a function of (prompt, schema).
pub type RouteFn = dyn Fn(&str, Option<&Schema>) -> FakeReply + Send + Sync;

/// How one purpose is answered.
pub enum Route {
    /// The same reply every time.
    Fixed(FakeReply),
    /// One reply per call, in order; the last one repeats.
    List(Vec<FakeReply>),
    /// A function of (prompt, schema).
    Func(Box<RouteFn>),
}

impl Route {
    pub fn fixed(reply: impl Into<FakeReply>) -> Route {
        Route::Fixed(reply.into())
    }
    pub fn list<R: Into<FakeReply>>(replies: impl IntoIterator<Item = R>) -> Route {
        Route::List(replies.into_iter().map(Into::into).collect())
    }
    pub fn func<R: Into<FakeReply>>(f: impl Fn(&str, Option<&Schema>) -> R + Send + Sync + 'static) -> Route {
        Route::Func(Box::new(move |prompt, schema| f(prompt, schema).into()))
    }
}

/// Answers by purpose: the exact purpose first, then the longest dotted prefix (`"engine"` answers
/// `"engine.round"`), then the default. A purpose with no route fails the test.
#[derive(Default)]
pub struct Routes {
    routes: HashMap<String, Route>,
    default: Option<Route>,
    used: Mutex<HashMap<String, usize>>,
}

impl Routes {
    pub fn new() -> Self {
        Routes::default()
    }

    pub fn route(mut self, purpose: &str, route: Route) -> Self {
        self.routes.insert(purpose.into(), route);
        self
    }

    pub fn default_route(mut self, route: Route) -> Self {
        self.default = Some(route);
        self
    }

    fn lookup(&self, purpose: &str) -> Option<(String, &Route)> {
        if let Some(route) = self.routes.get(purpose) {
            return Some((purpose.into(), route));
        }
        let parts: Vec<&str> = purpose.split('.').collect();
        for cut in (1..parts.len()).rev() {
            let prefix = parts[..cut].join(".");
            if let Some(route) = self.routes.get(&prefix) {
                return Some((prefix, route));
            }
        }
        self.default.as_ref().map(|r| ("\0default".to_string(), r))
    }

    pub fn respond(&self, purpose: &str, prompt: &str, schema: Option<&Schema>) -> FakeReply {
        let Some((key, route)) = self.lookup(purpose) else {
            let mut known: Vec<&String> = self.routes.keys().collect();
            known.sort();
            panic!("FakeLlm has no route for purpose {purpose:?}; known: {known:?}");
        };
        match route {
            Route::Fixed(reply) => reply.clone(),
            Route::Func(f) => f(prompt, schema),
            Route::List(replies) => {
                let mut used = self.used.lock().expect("not poisoned");
                let index = used.entry(key).or_insert(0);
                let reply = replies[(*index).min(replies.len() - 1)].clone();
                *index += 1;
                reply
            }
        }
    }
}
