//! `report.html`: the Markdown subset, citation links, the chart and action-mix data, and the Jinja template
//! (`templates/report.html`, the same file as the Python engine's) rendered with minijinja.

use std::collections::HashSet;
use std::sync::Arc;

use minijinja::value::Value;
use minijinja::{AutoEscape, Environment, Error, ErrorKind, Output, State};
use tellfold_engine::pytext::decimal_int;
use tellfold_models::json::py_round;
use tellfold_models::{ActionType, GodEvent, Post, Report, RoundStats, SimConfig};
use tellfold_store::world::World;

use crate::evidence::{collect_evidence, engagement, rounds_done, signed, total_actions, Evidence};
use crate::pyre;
use crate::report::{action_label, action_meaning, action_value, citation_line, RECOMMENDATIONS_NOTE};
use crate::ReportError;

const TEMPLATE: &str = include_str!("../templates/report.html");

/// Action types drawn in the mix chart (every type but `nothing`), in `ActionType` order.
pub const MIX_TYPES: [&str; 6] = ["post", "reply", "repost", "like", "dislike", "follow"];

/// `markupsafe.escape`: `& < > " '` as `&amp; &lt; &gt; &#34; &#39;`.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Links `#id` and `@handle` citations that exist in the run.
pub struct Refs {
    pub post_ids: HashSet<i64>,
    pub handles: HashSet<String>,
}

impl Refs {
    pub fn link(&self, html: &str) -> String {
        let posts = pyre::post_ref().replace_all(html, |c: &fancy_regex::Captures<'_>| {
            let whole = c.get(0).expect("match").as_str();
            match decimal_int(&c[1]) {
                Some(pid) if self.post_ids.contains(&pid) => format!("<a class=\"ref\" href=\"#p-{pid}\">#{pid}</a>"),
                _ => whole.to_string(),
            }
        });
        pyre::handle_ref()
            .replace_all(&posts, |c: &fancy_regex::Captures<'_>| {
                let h = &c[1];
                if self.handles.contains(h) {
                    format!("<a class=\"ref\" href=\"#agent-{h}\">@{h}</a>")
                } else {
                    c.get(0).expect("match").as_str().to_string()
                }
            })
            .into_owned()
    }
}

fn replace_group(re: &fancy_regex::Regex, text: &str, open: &str, close: &str) -> String {
    re.replace_all(text, |c: &fancy_regex::Captures<'_>| format!("{open}{}{close}", &c[1])).into_owned()
}

/// Inline Markdown: `code`, **bold**, *italic* and citation links, everything else escaped.
pub fn inline(text: &str, refs: &Refs) -> String {
    let mut out = String::new();
    let mut parts: Vec<&str> = Vec::new();
    let mut last = 0;
    for m in pyre::code_span().find_iter(text).flatten() {
        parts.push(&text[last..m.start()]);
        parts.push(m.as_str());
        last = m.end();
    }
    parts.push(&text[last..]);
    for part in parts {
        if part.chars().count() > 1 && part.starts_with('`') && part.ends_with('`') {
            out.push_str(&format!("<code>{}</code>", escape(&part[1..part.len() - 1])));
            continue;
        }
        let html = escape(part);
        let html = replace_group(pyre::bold(), &html, "<strong>", "</strong>");
        let html = replace_group(pyre::italic(), &html, "<em>", "</em>");
        out.push_str(&refs.link(&html));
    }
    out
}

/// Small Markdown subset: paragraphs, ### headings, bullet and numbered lists, quotes, bold, italic, code.
pub fn markdown(text: &str, refs: &Refs) -> String {
    let mut html: Vec<String> = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let mut items: Vec<String> = Vec::new();
    let mut list_tag = "";
    let flush = |html: &mut Vec<String>, para: &mut Vec<String>, items: &mut Vec<String>, list_tag: &mut &str| {
        if !para.is_empty() {
            html.push(format!("<p>{}</p>", inline(&para.join(" "), refs)));
            para.clear();
        }
        if !items.is_empty() {
            let lis: String = items.iter().map(|i| format!("<li>{i}</li>")).collect();
            html.push(format!("<{list_tag}>{lis}</{list_tag}>"));
            items.clear();
            *list_tag = "";
        }
    };
    for raw in tellfold_models::text::py_splitlines(text) {
        let line = tellfold_models::text::py_strip(raw);
        let capture = |re: &fancy_regex::Regex| re.captures(line).ok().flatten().map(|c| c[1].to_string());
        let bullet = capture(pyre::bullet());
        let numbered = capture(pyre::numbered());
        let heading = capture(pyre::heading());
        if line.is_empty() {
            flush(&mut html, &mut para, &mut items, &mut list_tag);
        } else if let Some(h) = heading {
            flush(&mut html, &mut para, &mut items, &mut list_tag);
            html.push(format!("<h4>{}</h4>", inline(&h, refs)));
        } else if bullet.is_some() || numbered.is_some() {
            let tag = if bullet.is_some() { "ul" } else { "ol" };
            if !para.is_empty() || (!items.is_empty() && list_tag != tag) {
                flush(&mut html, &mut para, &mut items, &mut list_tag);
            }
            list_tag = tag;
            items.push(inline(&bullet.or(numbered).expect("one of them"), refs));
        } else if line.starts_with('>') {
            flush(&mut html, &mut para, &mut items, &mut list_tag);
            html.push(format!("<blockquote>{}</blockquote>", inline(line.trim_start_matches(['>', ' ']), refs)));
        } else if let Some(last) = items.last_mut() {
            last.push(' ');
            last.push_str(&inline(line, refs));
        } else {
            para.push(line.to_string());
        }
    }
    flush(&mut html, &mut para, &mut items, &mut list_tag);
    html.join("\n")
}

fn cited_posts(report: &Report, ev: &Evidence, limit: usize) -> Vec<Post> {
    let mut texts: Vec<&str> = vec![&report.summary, &report.prediction];
    texts.extend(report.risks.iter().map(String::as_str));
    texts.extend(report.caveats.iter().map(String::as_str));
    texts.extend(report.sections.iter().map(|s| s.body_markdown.as_str()));
    let joined: Vec<String> = report.recommendations.iter().map(|r| r.evidence.join(" ")).collect();
    for (r, evidence) in report.recommendations.iter().zip(&joined) {
        texts.extend([r.why.as_str(), r.expected_effect.as_str(), evidence.as_str()]);
    }
    let mut seen: Vec<i64> = Vec::new();
    for text in texts {
        for c in pyre::post_ref().captures_iter(text).flatten() {
            if let Some(pid) = decimal_int(&c[1]) {
                if ev.posts.contains_key(&pid) && !seen.contains(&pid) {
                    seen.push(pid);
                }
            }
        }
    }
    seen.into_iter().take(limit).map(|pid| ev.posts[&pid].clone()).collect()
}

fn sentiment_chart(stats: &[RoundStats], events: &[GodEvent]) -> Value {
    if !stats.iter().any(|s| s.sentiment.is_some()) {
        return Value::from(());
    }
    let (width, height, left, right, top, bottom) = (480i64, 260i64, 40i64, 12i64, 18i64, 226i64);
    let right_edge = width - right;
    let rounds: Vec<i64> = stats.iter().map(|s| s.round).collect();
    let (lo, hi) = (*rounds.iter().min().expect("stats"), *rounds.iter().max().expect("stats"));
    let x_of = |r: i64| -> f64 {
        if hi == lo {
            (left + right_edge) as f64 / 2.0
        } else {
            left as f64 + (r - lo) as f64 / (hi - lo) as f64 * (right_edge - left) as f64
        }
    };
    let y_of = |v: f64| -> f64 { top as f64 + (1.0 - v.clamp(-1.0, 1.0)) / 2.0 * (bottom - top) as f64 };
    let mut points: Vec<Value> = Vec::new();
    let mut paths: Vec<String> = Vec::new();
    let mut segment: Vec<(f64, f64)> = Vec::new();
    for s in stats {
        let Some(sentiment) = s.sentiment else {
            segment.clear();
            continue;
        };
        let (x, y) = (py_round(x_of(s.round), 1), py_round(y_of(sentiment), 1));
        points.push(Value::from_iter([
            ("x", Value::from(x)),
            ("y", Value::from(y)),
            ("round", Value::from(s.round)),
            ("sentiment", Value::from(sentiment)),
        ]));
        if segment.is_empty() {
            paths.push(String::new());
        }
        segment.push((x, y));
        let d = segment.iter().map(|(x, y)| format!("{},{}", py_str(*x), py_str(*y))).collect::<Vec<_>>().join(" L");
        *paths.last_mut().expect("a path") = format!("M{d}");
    }
    let step = (rounds.len() / 10).max(1);
    let gridlines: Vec<Value> = [(1.0, "+1"), (0.5, "+0.5"), (0.0, "0"), (-0.5, "−0.5"), (-1.0, "−1")]
        .into_iter()
        .map(|(v, label)| Value::from(vec![Value::from(py_round(y_of(v), 1)), Value::from(label)]))
        .collect();
    let ticks: Vec<Value> = rounds
        .iter()
        .step_by(step)
        .map(|r| Value::from(vec![Value::from(py_round(x_of(*r), 1)), Value::from(format!("R{r}"))]))
        .collect();
    let event_marks: Vec<Value> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| lo <= e.round && e.round <= hi)
        .map(|(i, e)| {
            Value::from(vec![
                Value::from(py_round(x_of(e.round), 1)),
                Value::from(format!("E{}", i + 1)),
                Value::from(e.content.clone()),
            ])
        })
        .collect();
    Value::from_iter([
        ("width", Value::from(width)),
        ("height", Value::from(height)),
        ("left", Value::from(left)),
        ("right", Value::from(right_edge)),
        ("top", Value::from(top)),
        ("bottom", Value::from(bottom)),
        ("gridlines", Value::from(gridlines)),
        ("ticks", Value::from(ticks)),
        ("paths", Value::from(paths.into_iter().filter(|p| p.contains(" L")).collect::<Vec<_>>())),
        ("points", Value::from(points)),
        ("events", Value::from(event_marks)),
    ])
}

/// Python's `str()` of a float.
fn py_str(x: f64) -> String {
    tellfold_models::json::py_float_repr(x)
}

fn action_mix(stats: &[RoundStats]) -> Vec<Value> {
    let counted: Vec<(&RoundStats, Vec<(&str, i64)>)> = stats
        .iter()
        .map(|s| (s, MIX_TYPES.iter().map(|t| (*t, s.actions.get(*t).copied().unwrap_or(0))).collect()))
        .collect();
    let busiest = counted.iter().map(|(_, c)| c.iter().map(|(_, n)| n).sum::<i64>()).max().unwrap_or(0);
    let busiest = if busiest == 0 { 1 } else { busiest };
    counted
        .into_iter()
        .map(|(s, counts)| {
            let total: i64 = counts.iter().map(|(_, n)| n).sum();
            let segments: Vec<Value> = counts
                .iter()
                .filter(|(_, n)| *n != 0 && total != 0)
                .map(|(t, n)| {
                    Value::from(vec![
                        Value::from(*t),
                        Value::from(*n),
                        Value::from(py_round(100.0 * *n as f64 / total as f64, 2)),
                    ])
                })
                .collect();
            Value::from_iter([
                ("round", Value::from(s.round)),
                ("total", Value::from(total)),
                ("idle", Value::from(s.actions.get(ActionType::Nothing.value()).copied().unwrap_or(0))),
                ("width_pct", Value::from(py_round(100.0 * total as f64 / busiest as f64, 1))),
                ("segments", Value::from(segments)),
            ])
        })
        .collect()
}

/// A post as the template sees it (`posts.get(id)` works with integer keys).
fn post_value(p: &Post) -> Value {
    Value::from_serialize(p)
}

fn signed_filter(value: Value) -> String {
    if value.is_none() || value.is_undefined() {
        return "n/a".into();
    }
    signed(f64::try_from(value).ok())
}

/// Jinja's `truncate(length)` with its defaults (`killwords=False`, `end='...'`, `leeway=5`).
fn truncate_filter(value: String, length: Option<usize>) -> String {
    let length = length.unwrap_or(255);
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= length + 5 {
        return value;
    }
    let cut: String = chars[..length - 3].iter().collect();
    let head = match cut.rfind(' ') {
        Some(i) => &cut[..i],
        None => cut.as_str(),
    };
    format!("{head}...")
}

/// `"%.2f" | format(x)`: the printf subset the template uses.
fn format_filter(spec: String, args: minijinja::value::Rest<Value>) -> Result<String, Error> {
    let value = args.first().cloned().unwrap_or(Value::from(()));
    match spec.as_str() {
        "%.2f" => Ok(format!("{:.2}", f64::try_from(value)?)),
        other => Err(Error::new(ErrorKind::InvalidOperation, format!("unsupported format {other}"))),
    }
}

/// Jinja2's `capitalize`: the first character upper-cased, the rest lower-cased.
fn capitalize_filter(value: String) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// Writes values the way Jinja2 with markupsafe does: strings escaped with `markupsafe.escape`, safe strings as they
/// are, everything else as Python's `str()`.
fn formatter(out: &mut Output<'_>, state: &State<'_, '_>, value: &Value) -> Result<(), Error> {
    if value.is_safe() {
        return write!(out, "{value}").map_err(Error::from);
    }
    let text = if let Some(s) = value.as_str() {
        s.to_string()
    } else if value.kind() == minijinja::value::ValueKind::Number && value.as_i64().is_none() {
        py_str(f64::try_from(value.clone())?)
    } else if value.is_undefined() {
        String::new()
    } else {
        value.to_string()
    };
    match state.auto_escape() {
        AutoEscape::None => out.write_str(&text),
        _ => out.write_str(&escape(&text)),
    }
    .map_err(Error::from)
}

fn environment() -> Environment<'static> {
    let mut env = Environment::new();
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);
    env.set_auto_escape_callback(|name| if name.ends_with(".html") { AutoEscape::Html } else { AutoEscape::None });
    env.set_formatter(formatter);
    env.set_unknown_method_callback(minijinja_contrib::pycompat::unknown_method_callback);
    env.add_filter("signed", signed_filter);
    env.add_filter("truncate", truncate_filter);
    env.add_filter("format", format_filter);
    env.add_filter("capitalize", capitalize_filter);
    env.add_template("report.html", TEMPLATE).expect("the report template parses");
    env
}

fn report_value(report: &Report) -> Value {
    Value::from_serialize(report)
}

pub fn render_html(report: &Report, world: &World, config: &SimConfig) -> Result<String, ReportError> {
    let ev = collect_evidence(world)?;
    render_html_from(report, &ev, config)
}

pub fn render_html_from(report: &Report, ev: &Evidence, config: &SimConfig) -> Result<String, ReportError> {
    let refs = Arc::new(Refs {
        post_ids: ev.posts.keys().copied().collect(),
        handles: ev.people.values().map(|p| p.handle.clone()).collect(),
    });
    let people: Value = Value::from_iter(ev.people.iter().map(|(k, p)| (k.clone(), Value::from_serialize(p))));
    let posts: Value = Value::from_iter(ev.posts.iter().map(|(k, p)| (Value::from(*k), post_value(p))));
    let handles: std::collections::HashMap<String, String> =
        ev.people.iter().map(|(k, p)| (k.clone(), p.handle.clone())).collect();
    let events: Vec<GodEvent> = ev.events.iter().map(|i| i.event.clone()).collect();
    let md_refs = refs.clone();
    let inline_refs = refs.clone();
    let action_names: Vec<(&str, Value)> = [
        tellfold_models::RecommendationAction::Keep,
        tellfold_models::RecommendationAction::Change,
        tellfold_models::RecommendationAction::Add,
        tellfold_models::RecommendationAction::Drop,
        tellfold_models::RecommendationAction::Test,
    ]
    .into_iter()
    .map(|a| (action_value(a), Value::from(action_label(a))))
    .collect();
    let action_meanings: Vec<(&str, Value)> = [
        tellfold_models::RecommendationAction::Keep,
        tellfold_models::RecommendationAction::Change,
        tellfold_models::RecommendationAction::Add,
        tellfold_models::RecommendationAction::Drop,
        tellfold_models::RecommendationAction::Test,
    ]
    .into_iter()
    .map(|a| (action_value(a), Value::from(action_meaning(a))))
    .collect();
    let event_values: Vec<Value> = ev
        .events
        .iter()
        .map(|e| {
            Value::from_iter([
                ("event", Value::from_serialize(&e.event)),
                ("sentiment_before", e.sentiment_before.map_or(Value::from(()), Value::from)),
                ("sentiment_after", e.sentiment_after.map_or(Value::from(()), Value::from)),
                ("posts_before", Value::from(e.posts_before)),
                ("posts_after", Value::from(e.posts_after)),
            ])
        })
        .collect();
    let threads: Vec<Value> = ev
        .threads
        .iter()
        .map(|t| {
            Value::from_iter([
                ("root", post_value(&t.root)),
                ("replies", Value::from(t.replies.iter().map(post_value).collect::<Vec<_>>())),
                ("size", Value::from(t.size)),
            ])
        })
        .collect();
    let directory: Vec<Value> = ev
        .agents
        .iter()
        .map(|a| {
            Value::from_iter([
                ("persona", Value::from_serialize(&a.persona)),
                ("followers", Value::from(a.followers)),
                ("posts", Value::from(a.posts)),
                ("engagement", Value::from(a.engagement)),
            ])
        })
        .collect();
    let stats: Vec<Value> = ev.stats.iter().map(Value::from_serialize).collect();
    let key_voices: Vec<Value> =
        report.key_voices.iter().filter_map(|v| ev.people.get(v)).map(Value::from_serialize).collect();
    let tiles = vec![
        Value::from(vec![Value::from("Agents"), Value::from(ev.people.len())]),
        Value::from(vec![Value::from("Rounds"), Value::from(rounds_done(ev))]),
        Value::from(vec![Value::from("Posts"), Value::from(ev.posts.len())]),
        Value::from(vec![Value::from("Actions"), Value::from(total_actions(ev))]),
    ];
    let ctx = Value::from_iter([
        ("report", report_value(report)),
        ("config", Value::from_serialize(config)),
        ("citations", citation_line(report.citation_audit.as_ref()).map_or(Value::from(()), Value::from)),
        ("md", Value::from_function(move |text: String| Value::from_safe_string(markdown(&text, &md_refs)))),
        ("inline", Value::from_function(move |text: String| Value::from_safe_string(inline(&text, &inline_refs)))),
        ("people", people),
        ("posts", posts),
        ("key_voices", Value::from(key_voices)),
        ("action_labels", Value::from_iter(action_names)),
        ("action_meanings", Value::from_iter(action_meanings)),
        ("recommendations_note", Value::from(RECOMMENDATIONS_NOTE)),
        (
            "addressee",
            Value::from_function(move |a: String| match handles.get(&a) {
                Some(h) => format!("@{h}"),
                None => a,
            }),
        ),
        ("tiles", Value::from(tiles)),
        ("chart", sentiment_chart(&ev.stats, &events)),
        ("stats", Value::from(stats)),
        ("mix", Value::from(action_mix(&ev.stats))),
        ("mix_types", Value::from(MIX_TYPES.to_vec())),
        ("events", Value::from(event_values)),
        ("cited", Value::from(cited_posts(report, ev, 24).iter().map(post_value).collect::<Vec<_>>())),
        ("threads", Value::from(threads)),
        ("directory", Value::from(directory)),
        (
            "engagement",
            Value::from_function(|p: Value| -> Result<i64, Error> {
                let post: Post = serde_json::from_value(
                    serde_json::to_value(&p).map_err(|e| Error::new(ErrorKind::InvalidOperation, e.to_string()))?,
                )
                .map_err(|e| Error::new(ErrorKind::InvalidOperation, e.to_string()))?;
                Ok(engagement(&post))
            }),
        ),
    ]);
    let env = environment();
    Ok(env.get_template("report.html").expect("added").render(ctx)?)
}
