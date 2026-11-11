//! The live progress view of `tellfold run` and `tellfold branch`: finished stages with their time, the current stage
//! with a spinner, the latest notes and the round table (`RunView` and `pipeline_listener` in `tellfold/cli.py`).

use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::Value;
use tellfold_models::{RoundStats, SimConfig};

use crate::term::{Justify, Table, Text};
use crate::ui::{Block, Spinner};

/// `-`, or the score with its sign, green above 0.1, red below -0.1, yellow between.
pub fn fmt_sentiment(value: Option<f64>) -> Text {
    match value {
        None => Text::styled("-", "dim"),
        Some(v) => {
            let style = if v > 0.1 {
                "green"
            } else if v < -0.1 {
                "red"
            } else {
                "yellow"
            };
            Text::styled(&format!("{v:+.2}"), style)
        }
    }
}

pub fn round_table(stats: &[RoundStats], total_rounds: i64) -> Table {
    let done = stats.iter().filter(|s| s.round > 0).count(); // round 0 holds the opening posts
    let mut table = Table::new().title(&format!("Rounds ({done}/{total_rounds})"));
    for (header, justify) in [
        ("Round", Justify::Right),
        ("Active", Justify::Right),
        ("Posts", Justify::Right),
        ("Action mix", Justify::Left),
        ("Sentiment", Justify::Right),
        ("Calls", Justify::Right),
        ("LLM s", Justify::Right),
    ] {
        table.add_column(header, justify);
    }
    for s in stats {
        let mut actions: Vec<(&String, &i64)> = s.actions.iter().collect();
        actions.sort_by_key(|(_, v)| -**v); // stable, like sorted(key=-count)
        let mix = actions.iter().filter(|(_, v)| **v != 0).map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>();
        let mix = if mix.is_empty() { "-".to_string() } else { mix.join(", ") };
        table.add_row(vec![
            Text::from_markup(&s.round.to_string()),
            Text::from_markup(&s.active_agents.to_string()),
            Text::from_markup(&s.new_posts.to_string()),
            Text::from_markup(&mix),
            fmt_sentiment(s.sentiment),
            Text::from_markup(&s.llm_calls.to_string()),
            Text::from_markup(&format!("{:.0}", s.llm_seconds)),
        ]);
    }
    table
}

#[derive(Debug, Default)]
pub struct RunView {
    pub total_rounds: i64,
    finished: Vec<(String, f64)>,
    current: Option<String>,
    started: Option<Instant>,
    detail: String,
    notes: Vec<String>,
    stats: Vec<RoundStats>,
}

impl RunView {
    pub fn new(total_rounds: i64) -> Self {
        RunView { total_rounds, ..Default::default() }
    }

    /// Close the current stage (if any) and start a new one.
    pub fn begin(&mut self, label: &str) {
        if self.current.as_deref() == Some(label) {
            return;
        }
        self.end();
        self.current = Some(label.into());
        self.detail.clear();
        self.started = Some(Instant::now());
    }

    pub fn end(&mut self) {
        if let Some(label) = self.current.take() {
            let secs = self.started.map_or(0.0, |t| t.elapsed().as_secs_f64());
            self.finished.push((label, secs));
        }
    }

    pub fn set_detail(&mut self, text: &str) {
        self.detail = text.into();
    }

    pub fn note(&mut self, text: &str) {
        let keep = self.notes.len().saturating_sub(3);
        self.notes.drain(..keep);
        self.notes.push(text.into());
    }

    pub fn render(&self) -> Vec<Block> {
        let mut lines: Vec<Block> = self
            .finished
            .iter()
            .map(|(label, secs)| {
                // Text.assemble: each part keeps its own style
                let mut text = Text::new();
                text.push("  ✓ ", "green");
                text.push(label, "");
                text.push(&format!("  {secs:.1}s"), "dim");
                Block::Text(text)
            })
            .collect();
        if let Some(current) = &self.current {
            let detail = if self.detail.is_empty() { String::new() } else { format!("  {}", self.detail) };
            let mut text = Text::plain(current);
            text.push(&detail, "dim");
            lines.push(Block::Spinner(Spinner::new(text, "cyan")));
        }
        lines.extend(self.notes.iter().map(|n| Block::Text(Text::styled(&format!("    {n}"), "dim"))));
        if !self.stats.is_empty() {
            lines.push(Block::Table(Box::new(round_table(&self.stats, self.total_rounds))));
        }
        lines
    }
}

fn stage_label(stage: &str, config: &SimConfig) -> Option<String> {
    Some(match stage {
        "reading" => "Reading seed documents".into(),
        "graph" => "Extracting knowledge graph".into(),
        "personas" => format!("Creating {} personas", config.num_agents),
        "seeding" => "Seeding follows and opening posts".into(),
        "rounds" => format!("Simulating {} rounds", config.rounds),
        "report" => "Writing the report".into(),
        _ => return None,
    })
}

/// Python's `str()` of an event payload value, for the scalars the view shows.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(_) | Value::Bool(_) => Some(tellfold_models::json::py_str(value)),
        _ => None,
    }
}

fn text_of(value: Option<&Value>) -> String {
    value.map_or("None".into(), tellfold_models::json::py_str)
}

/// Feeds `run_pipeline`'s events into the view.
pub fn on_event(view: &Arc<Mutex<RunView>>, config: &SimConfig, name: &str, data: &Value) {
    let mut view = view.lock().expect("not poisoned");
    match name {
        "stage" => {
            let stage = data.get("stage").map(tellfold_models::json::py_str).unwrap_or_default();
            if let Some(label) = stage_label(&stage, config) {
                view.begin(&label);
            }
            view.set_detail(&data.get("detail").map(tellfold_models::json::py_str).unwrap_or_default());
            if let Some(estimate) = data.get("estimated_calls") {
                // A branch or a reused cast reads no documents, so its estimate has no chunk count.
                let chunks = match data.get("chunks") {
                    Some(c) if !c.is_null() => format!("{} chunks; ", tellfold_models::json::py_str(c)),
                    _ => String::new(),
                };
                let estimate = tellfold_models::json::py_str(estimate);
                view.note(&format!("{chunks}about {estimate} model calls this run if none need a retry"));
            }
            if let Some(entities) = data.get("entities") {
                let relations = data.get("relations").map_or("0".into(), tellfold_models::json::py_str);
                view.note(&format!(
                    "graph: {} entities, {relations} relations",
                    tellfold_models::json::py_str(entities)
                ));
            }
        }
        "round_done" => {
            let Ok(stats) = serde_json::from_value::<RoundStats>(data.clone()) else { return };
            view.stats.retain(|s| s.round != stats.round);
            let round = stats.round;
            view.stats.push(stats);
            if round > 0 {
                view.set_detail(&format!("round {round} of {} done", config.rounds));
            }
        }
        "run_done" | "run_stopped" | "run_failed" => view.end(),
        "status" | "posts_added" => {}
        "event_injected" => view.note(&format!("round {}: breaking news injected", text_of(data.get("round")))),
        _ => {
            let scalars = data
                .as_object()
                .map(|o| o.iter().filter_map(|(k, v)| scalar(v).map(|v| format!("{k}={v}"))).collect::<Vec<_>>())
                .unwrap_or_default()
                .join(", ");
            let scalars: String = scalars.chars().take(80).collect();
            let label = name.replace('_', " ");
            let message = if scalars.is_empty() { label } else { format!("{label}: {scalars}") };
            if name.contains("fail") || name.contains("error") {
                view.note(&format!("warning: {message}"));
            } else {
                view.set_detail(&message);
            }
        }
    }
}
