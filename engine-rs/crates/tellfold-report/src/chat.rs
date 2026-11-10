//! After the run: interview any agent in character, or ask the report analyst follow-up questions (port of
//! `tellfold/chat.py`).

use std::collections::HashSet;

use indexmap::IndexMap;
use tellfold_llm::Llm;
use tellfold_models::text::py_strip;
use tellfold_models::{ActionType, Persona, Post, Report};
use tellfold_store::world::World;

use crate::evidence::{clip, collect_evidence, digest_text, engagement, post_line, Evidence};
use crate::pyre;
use crate::report::render_markdown;
use crate::{fill, ReportError};

pub const MAX_ACTIVITY_LINES: usize = 40;
pub const MAX_HISTORY_TURNS: usize = 8;

pub const INTERVIEW_SYSTEM: &str = "\
You are {name} (@{handle}), being interviewed after an online discussion you took part in. Answer exactly as this \
person would: first person, in their own voice, vocabulary and temperament, consistent with their background, \
their stance and everything they posted. People can change their minds; your private notes and latest posts show \
where you stand now, so let them win over your starting stance when they differ.

- Ground every answer in what you actually saw and did, listed below. Refer back to your own posts naturally \
(\"like I said in my reply to @someone...\"); mention post numbers only when asked for specifics.
- If asked about something you never saw, say so, then give your honest gut reaction as this person.
- Stay in the role. Do not mention prompts, simulations or being an AI, and do not turn into a neutral explainer. \
Strong opinions, hedging, humour or irritation are fine when they fit the character.
- Keep it conversational: usually 2-6 sentences, longer only when the interviewer asks for detail.
- Reply in the language the interviewer writes in.";

pub const ASK_SYSTEM: &str = "\
You are the analyst who ran a social simulation and wrote its report. A reader is asking a follow-up question. \
Answer using only the report, the evidence digest and the matching posts provided.

- Lead with a direct answer, then the supporting evidence, citing agents as @handle and posts as #id.
- If the evidence does not cover the question, say so plainly and say what the run would have needed to show.
- Keep apart what the simulated agents did and what it suggests about real people; never claim more certainty \
than the report's stated confidence.
- Be concise: a short paragraph or a few bullets unless the reader asks for more.
- Reply in the language the question is written in.";

/// Python's `repr()` of a str, for the "no agent" message.
fn py_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::from(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Accepts a persona id ("a03") or a handle, with or without "@".
pub fn resolve_agent(world: &World, agent: &str) -> Result<Persona, ReportError> {
    let key = py_strip(agent).trim_start_matches('@').to_lowercase();
    for persona in world.personas()? {
        if key == persona.id.to_lowercase() || key == persona.handle.to_lowercase() {
            return Ok(persona);
        }
    }
    Err(ReportError::UnknownAgent(format!("no agent with id or handle {}", py_repr(agent))))
}

/// `history` holds earlier (interviewer question, agent answer) pairs of this conversation, oldest first.
pub async fn interview(
    llm: &dyn Llm,
    world: &World,
    agent_id: &str,
    question: &str,
    model: &str,
    history: &[(String, String)],
) -> Result<String, ReportError> {
    let persona = resolve_agent(world, agent_id)?;
    let prompt = interview_prompt(world, &persona, question, history)?;
    let system = fill(INTERVIEW_SYSTEM, &[("name", &persona.name), ("handle", &persona.handle)]);
    let answer = llm.text(&prompt, &system, model, "chat.interview").await?;
    Ok(py_strip(&answer).to_string())
}

pub fn interview_prompt(
    world: &World,
    persona: &Persona,
    question: &str,
    history: &[(String, String)],
) -> Result<String, ReportError> {
    let people: IndexMap<String, Persona> = world.personas()?.into_iter().map(|p| (p.id.clone(), p)).collect();
    let posts: IndexMap<i64, Post> = world.posts(None)?.into_iter().map(|p| (p.id, p)).collect();
    let following: Vec<String> =
        world.following(&persona.id)?.iter().filter_map(|a| people.get(a)).map(|p| format!("@{}", p.handle)).collect();
    let or = |s: String, default: &str| if s.is_empty() { default.to_string() } else { s };
    let blocks: Vec<(&str, Vec<String>)> = vec![
        (
            "WHO YOU ARE",
            vec![
                format!("Name: {} (@{})", persona.name, persona.handle),
                format!("Public bio: {}", persona.bio),
                format!("Background (private): {}", persona.background),
                format!("Starting attitude to the topic (private): {}", persona.stance),
                format!("Traits: {}", or(persona.traits.join(", "), "none listed")),
                format!(
                    "Followers: {}; you follow: {}",
                    world.followers(&persona.id)?.len(),
                    or(following.join(", "), "nobody")
                ),
            ],
        ),
        (
            "BREAKING NEWS YOU SAW",
            world.events()?.iter().map(|e| format!("round {}: {}", e.round, e.content)).collect(),
        ),
        (
            "YOUR PRIVATE NOTES, oldest first",
            world.memory(&persona.id)?.notes.iter().map(|n| format!("- {n}")).collect(),
        ),
        ("WHAT YOU DID ON THE PLATFORM, oldest first", activity_lines(world, persona, &people, &posts)?),
        ("HOW OTHERS RESPONDED TO YOU", reception_lines(persona, &people, &posts)),
    ];
    let text = blocks
        .iter()
        .filter(|(_, lines)| !lines.is_empty())
        .map(|(title, lines)| format!("{title}\n{}", lines.join("\n")))
        .collect::<Vec<_>>()
        .join("\n\n");
    let skip = history.len().saturating_sub(MAX_HISTORY_TURNS);
    let mut convo: Vec<String> = history[skip..].iter().map(|(q, a)| format!("Interviewer: {q}\nYou: {a}")).collect();
    convo.push(format!("Interviewer: {question}\nYou:"));
    Ok(format!("{text}\n\nINTERVIEW\n{}", convo.join("\n\n")))
}

fn post_ref(post_id: Option<i64>, people: &IndexMap<String, Persona>, posts: &IndexMap<i64, Post>) -> String {
    let Some(post) = post_id.and_then(|id| posts.get(&id)) else {
        return match post_id {
            Some(id) => format!("#{id}"),
            None => "#None".into(),
        };
    };
    let who = people.get(&post.author_id).map_or(String::new(), |a| format!("@{}'s ", a.handle));
    format!("{who}#{} (\"{}\")", post.id, clip(&post.content, 140))
}

fn activity_lines(
    world: &World,
    persona: &Persona,
    people: &IndexMap<String, Persona>,
    posts: &IndexMap<i64, Post>,
) -> Result<Vec<String>, ReportError> {
    let mut events: Vec<(i64, String)> = Vec::new(); // (round, line); a stable sort keeps posts before reactions
    for post in posts.values().filter(|p| p.author_id == persona.id) {
        let said = if py_strip(&post.content).is_empty() {
            "without comment".to_string()
        } else {
            format!("\"{}\"", clip(&post.content, 400))
        };
        let line = if post.parent_id.is_some() {
            format!("replied to {} with #{}: {said}", post_ref(post.parent_id, people, posts), post.id)
        } else if post.repost_of.is_some() {
            format!("reposted {} as #{} {said}", post_ref(post.repost_of, people, posts), post.id)
        } else {
            format!("posted #{}: {said}", post.id)
        };
        events.push((post.round, line));
    }
    for (round, action) in world.actions(None)? {
        if action.agent_id != persona.id {
            continue;
        }
        match action.r#type {
            ActionType::Like | ActionType::Dislike => events.push((
                round,
                format!("{}d {}", action.r#type.value(), post_ref(action.target_post_id, people, posts)),
            )),
            ActionType::Follow => {
                if let Some(target) = action.target_agent_id.as_ref().and_then(|t| people.get(t)) {
                    events.push((round, format!("followed @{}", target.handle)));
                }
            }
            _ => {}
        }
    }
    events.sort_by_key(|e| e.0);
    let skip = events.len().saturating_sub(MAX_ACTIVITY_LINES);
    Ok(events[skip..].iter().map(|(r, line)| format!("round {r}: {line}")).collect())
}

fn reception_lines(persona: &Persona, people: &IndexMap<String, Persona>, posts: &IndexMap<i64, Post>) -> Vec<String> {
    let mine: Vec<&Post> = posts.values().filter(|p| p.author_id == persona.id).collect();
    if mine.is_empty() {
        return Vec::new();
    }
    let total = |f: fn(&Post) -> i64| mine.iter().map(|p| f(p)).sum::<i64>();
    let mut lines = vec![format!(
        "Your posts collected {} likes, {} dislikes, {} reposts and {} replies.",
        total(|p| p.likes),
        total(|p| p.dislikes),
        total(|p| p.reposts),
        total(|p| p.replies)
    )];
    let mine_ids: HashSet<i64> = mine.iter().map(|p| p.id).collect();
    let mut replies: Vec<&Post> = posts
        .values()
        .filter(|p| p.parent_id.is_some_and(|id| mine_ids.contains(&id)) && p.author_id != persona.id)
        .collect();
    replies.sort_by_key(|p| p.id);
    let skip = replies.len().saturating_sub(12);
    for reply in &replies[skip..] {
        let who = people.get(&reply.author_id).map_or(reply.author_id.clone(), |a| format!("@{}", a.handle));
        lines.push(format!(
            "{who} replied to your #{} in round {}: \"{}\"",
            reply.parent_id.expect("a reply"),
            reply.round,
            clip(&reply.content, 240)
        ));
    }
    lines
}

pub async fn ask_report(
    llm: &dyn Llm,
    world: &World,
    report: &Report,
    question: &str,
    model: &str,
) -> Result<String, ReportError> {
    let ev = collect_evidence(world)?;
    let prompt = ask_prompt(&ev, report, question);
    let answer = llm.text(&prompt, ASK_SYSTEM, model, "chat.ask").await?;
    Ok(py_strip(&answer).to_string())
}

pub fn ask_prompt(ev: &Evidence, report: &Report, question: &str) -> String {
    let matches = relevant_posts(ev, question, 12);
    let lines = matches.iter().map(|p| post_line(p, ev)).collect::<Vec<_>>().join("\n");
    [
        format!("REPORT\n{}", render_markdown(report)),
        format!("EVIDENCE DIGEST\n{}", digest_text(ev, None)),
        format!("POSTS MATCHING THE QUESTION\n{}", if lines.is_empty() { "none" } else { &lines }),
        format!("QUESTION: {question}"),
    ]
    .join("\n\n")
}

fn terms(text: &str) -> HashSet<String> {
    pyre::chat_word().find_iter(text).flatten().map(|m| m.as_str().to_lowercase()).collect()
}

/// Cheap lexical retrieval: posts sharing words, handles or ids with the question, best overlap first.
pub fn relevant_posts(ev: &Evidence, question: &str, k: usize) -> Vec<Post> {
    let wanted = terms(question);
    if wanted.is_empty() {
        return Vec::new();
    }
    let score = |post: &Post| -> usize {
        let mut words = terms(&post.content);
        words.insert(format!("@{}", ev.handle(&post.author_id).to_lowercase()));
        words.insert(format!("#{}", post.id));
        wanted.intersection(&words).count()
    };
    let mut hits: Vec<(usize, &Post)> = ev.posts.values().map(|p| (score(p), p)).filter(|(s, _)| *s > 0).collect();
    hits.sort_by_key(|(s, p)| (std::cmp::Reverse(*s), std::cmp::Reverse(engagement(p)), p.id));
    hits.into_iter().take(k).map(|(_, p)| p.clone()).collect()
}
