//! The evidence digest: what the report and the report chat reason over, computed from `world.db`.

use indexmap::IndexMap;
use tellfold_models::json::{py_round, py_sum};
use tellfold_models::text::py_strip;
use tellfold_models::{ActionType, GodEvent, KnowledgeGraph, Persona, Post, RoundStats};
use tellfold_store::world::{World, WorldError};

use tellfold_engine::pytext::{char_len, rstrip, squash};
use tellfold_models::text::py_prefix;

pub const SNIPPET_CHARS: usize = 280;

#[derive(Debug, Clone)]
pub struct AgentStats {
    pub persona: Persona,
    pub followers: i64,
    /// authored posts, replies and reposts
    pub posts: i64,
    /// summed engagement of authored posts
    pub engagement: i64,
}

#[derive(Debug, Clone)]
pub struct Thread {
    pub root: Post,
    pub replies: Vec<Post>,
    /// all descendants, not only the ones kept in `replies`
    pub size: usize,
}

#[derive(Debug, Clone)]
pub struct StanceShift {
    pub persona: Persona,
    pub first: Post,
    pub last: Post,
    pub posts: usize,
    pub last_note: String,
}

#[derive(Debug, Clone)]
pub struct EventImpact {
    pub event: GodEvent,
    pub sentiment_before: Option<f64>,
    pub sentiment_after: Option<f64>,
    /// mean new posts per round in the window
    pub posts_before: f64,
    pub posts_after: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Evidence {
    pub people: IndexMap<String, Persona>,
    pub posts: IndexMap<i64, Post>,
    pub stats: Vec<RoundStats>,
    /// most-followed first
    pub agents: Vec<AgentStats>,
    pub top_posts: Vec<Post>,
    pub threads: Vec<Thread>,
    pub shifts: Vec<StanceShift>,
    pub events: Vec<EventImpact>,
    pub action_totals: IndexMap<String, i64>,
    /// post id -> scored sentiment, when scored
    pub post_sentiment: IndexMap<i64, f64>,
}

impl Evidence {
    pub fn handle<'a>(&'a self, agent_id: &'a str) -> &'a str {
        self.people.get(agent_id).map_or(agent_id, |p| p.handle.as_str())
    }
}

pub fn engagement(post: &Post) -> i64 {
    post.likes + 2 * post.reposts + post.replies - post.dislikes
}

/// `math.fsum`: the correctly rounded sum (Shewchuk's algorithm, as CPython implements it).
pub fn fsum(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut partials: Vec<f64> = Vec::new();
    for mut x in values {
        let mut i = 0;
        for j in 0..partials.len() {
            let mut y = partials[j];
            if x.abs() < y.abs() {
                std::mem::swap(&mut x, &mut y);
            }
            let hi = x + y;
            let lo = y - (hi - x);
            if lo != 0.0 {
                partials[i] = lo;
                i += 1;
            }
            x = hi;
        }
        partials.truncate(i);
        partials.push(x);
    }
    let mut n = partials.len();
    if n == 0 {
        return 0.0;
    }
    n -= 1;
    let mut hi = partials[n];
    let mut lo = 0.0;
    while n > 0 {
        let x = hi;
        n -= 1;
        let y = partials[n];
        hi = x + y;
        let yr = hi - x;
        lo = y - yr;
        if lo != 0.0 {
            break;
        }
    }
    if n > 0 && ((lo < 0.0 && partials[n - 1] < 0.0) || (lo > 0.0 && partials[n - 1] > 0.0)) {
        let y = lo * 2.0;
        let x = hi + y;
        let yr = x - hi;
        if y == yr {
            hi = x;
        }
    }
    hi
}

/// `statistics.fmean`.
pub fn fmean(values: &[f64]) -> f64 {
    fsum(values.iter().copied()) / values.len() as f64
}

/// Pure digest of the run: everything the report and the report chat reason over.
pub fn collect_evidence(world: &World) -> Result<Evidence, WorldError> {
    collect_evidence_with(world, 8, 2)
}

pub fn collect_evidence_with(world: &World, top_k: usize, event_window: i64) -> Result<Evidence, WorldError> {
    let people: IndexMap<String, Persona> = world.personas()?.into_iter().map(|p| (p.id.clone(), p)).collect();
    let posts: IndexMap<i64, Post> = world.posts(None)?.into_iter().map(|p| (p.id, p)).collect();
    let mut stats = world.round_stats()?;
    stats.sort_by_key(|s| s.round);
    let mut totals: IndexMap<String, i64> = IndexMap::new();
    for s in &stats {
        for (kind, count) in &s.actions {
            *totals.entry(kind.clone()).or_default() += count;
        }
    }
    let mut events = world.events()?;
    events.sort_by_key(|e| e.round);
    Ok(Evidence {
        agents: agent_stats(world, &people, &posts)?,
        top_posts: top_posts(&posts, top_k),
        threads: threads(&posts, 4, 6),
        shifts: stance_shifts(world, &people, &posts, 12)?,
        events: events.into_iter().map(|e| event_impact(e, &stats, event_window)).collect(),
        action_totals: totals,
        post_sentiment: world.post_sentiments()?,
        people,
        posts,
        stats,
    })
}

fn agent_stats(
    world: &World,
    people: &IndexMap<String, Persona>,
    posts: &IndexMap<i64, Post>,
) -> Result<Vec<AgentStats>, WorldError> {
    let mut authored: IndexMap<&str, Vec<&Post>> = IndexMap::new();
    for post in posts.values() {
        authored.entry(post.author_id.as_str()).or_default().push(post);
    }
    let mut rows = Vec::with_capacity(people.len());
    for p in people.values() {
        let mine = authored.get(p.id.as_str()).map(Vec::as_slice).unwrap_or(&[]);
        rows.push(AgentStats {
            persona: p.clone(),
            followers: world.followers(&p.id)?.len() as i64,
            posts: mine.len() as i64,
            engagement: mine.iter().map(|x| engagement(x)).sum(),
        });
    }
    rows.sort_by(|a, b| {
        (-a.followers, -a.engagement, &a.persona.id).cmp(&(-b.followers, -b.engagement, &b.persona.id))
    });
    Ok(rows)
}

fn top_posts(posts: &IndexMap<i64, Post>, k: usize) -> Vec<Post> {
    let mut originals: Vec<&Post> =
        posts.values().filter(|p| p.repost_of.is_none() && !py_strip(&p.content).is_empty()).collect();
    originals.sort_by_key(|p| (-engagement(p), p.id));
    originals.into_iter().take(k).cloned().collect()
}

fn children(posts: &IndexMap<i64, Post>) -> IndexMap<i64, Vec<&Post>> {
    let mut children: IndexMap<i64, Vec<&Post>> = IndexMap::new();
    for post in posts.values() {
        if let Some(parent) = post.parent_id {
            children.entry(parent).or_default().push(post);
        }
    }
    children
}

fn descendants<'a>(root_id: i64, children: &IndexMap<i64, Vec<&'a Post>>) -> Vec<&'a Post> {
    let mut found: Vec<&Post> = Vec::new();
    let mut stack = vec![root_id];
    while let Some(id) = stack.pop() {
        for child in children.get(&id).map(Vec::as_slice).unwrap_or(&[]) {
            found.push(child);
            stack.push(child.id);
        }
    }
    found.sort_by_key(|p| p.id);
    found
}

fn threads(posts: &IndexMap<i64, Post>, limit: usize, max_replies: usize) -> Vec<Thread> {
    let children = children(posts);
    let mut threads: Vec<Thread> = posts
        .values()
        .filter(|p| p.parent_id.is_none() && p.repost_of.is_none() && children.contains_key(&p.id))
        .map(|p| {
            let replies = descendants(p.id, &children);
            Thread {
                root: p.clone(),
                size: replies.len(),
                replies: replies.into_iter().take(max_replies).cloned().collect(),
            }
        })
        .collect();
    threads.sort_by_key(|t| (std::cmp::Reverse(t.size), -engagement(&t.root), t.root.id));
    threads.truncate(limit);
    threads
}

fn stance_shifts(
    world: &World,
    people: &IndexMap<String, Persona>,
    posts: &IndexMap<i64, Post>,
    limit: usize,
) -> Result<Vec<StanceShift>, WorldError> {
    let mut sorted: Vec<&Post> = posts.values().collect();
    sorted.sort_by_key(|p| p.id);
    let mut by_author: IndexMap<&str, Vec<&Post>> = IndexMap::new();
    for post in sorted {
        if !py_strip(&post.content).is_empty() {
            by_author.entry(post.author_id.as_str()).or_default().push(post);
        }
    }
    let mut shifts = Vec::new();
    for (agent_id, authored) in by_author {
        let Some(persona) = people.get(agent_id) else { continue };
        if authored.len() < 2 {
            continue;
        }
        let notes = world.memory(agent_id)?.notes;
        shifts.push(StanceShift {
            persona: persona.clone(),
            first: authored[0].clone(),
            last: authored[authored.len() - 1].clone(),
            posts: authored.len(),
            last_note: notes.last().cloned().unwrap_or_default(),
        });
    }
    shifts.sort_by(|a, b| {
        b.posts
            .cmp(&a.posts)
            .then(b.persona.influence.partial_cmp(&a.persona.influence).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.persona.id.cmp(&b.persona.id))
    });
    shifts.truncate(limit);
    Ok(shifts)
}

fn mean_sentiment(stats: &[&RoundStats]) -> Option<f64> {
    let values: Vec<f64> = stats.iter().filter_map(|s| s.sentiment).collect();
    (!values.is_empty()).then(|| py_round(fmean(&values), 3))
}

fn event_impact(event: GodEvent, stats: &[RoundStats], window: i64) -> EventImpact {
    let before: Vec<&RoundStats> =
        stats.iter().filter(|s| event.round - window <= s.round && s.round < event.round).collect();
    let after: Vec<&RoundStats> =
        stats.iter().filter(|s| event.round <= s.round && s.round < event.round + window).collect();
    let posts_mean = |rows: &[&RoundStats]| {
        if rows.is_empty() {
            0.0
        } else {
            py_round(fmean(&rows.iter().map(|s| s.new_posts as f64).collect::<Vec<_>>()), 1)
        }
    };
    EventImpact {
        sentiment_before: mean_sentiment(&before),
        sentiment_after: mean_sentiment(&after),
        posts_before: posts_mean(&before),
        posts_after: posts_mean(&after),
        event,
    }
}

// ---------------------------------------------------------------- digest text

/// Squashes whitespace and trims to `limit` code points with "…".
pub fn clip(text: &str, limit: usize) -> String {
    let text = squash(text);
    if char_len(&text) <= limit {
        text
    } else {
        format!("{}…", rstrip(py_prefix(&text, limit - 1)))
    }
}

/// `f"{value:+.2f}"`, or "n/a".
pub fn signed(value: Option<f64>) -> String {
    match value {
        None => "n/a".into(),
        Some(v) => format!("{v:+.2}"),
    }
}

/// Python's `str()` of a float.
pub fn py_float(value: f64) -> String {
    tellfold_models::json::py_float_repr(value)
}

/// One compact line per post, the citation format the LLM is asked to reuse (#id, @handle).
pub fn post_line(post: &Post, ev: &Evidence) -> String {
    let link = if let Some(parent) = post.parent_id {
        format!(" reply to #{parent}")
    } else if let Some(original) = post.repost_of {
        format!(" repost of #{original}")
    } else {
        String::new()
    };
    let counts = format!(
        "[{} likes, {} dislikes, {} reposts, {} replies]",
        post.likes, post.dislikes, post.reposts, post.replies
    );
    let body = if py_strip(&post.content).is_empty() {
        "(no comment)".to_string()
    } else {
        clip(&post.content, SNIPPET_CHARS)
    };
    format!("#{} r{} @{}{link} {counts}: \"{body}\"", post.id, post.round, ev.handle(&post.author_id))
}

fn timeline_lines(ev: &Evidence) -> Vec<String> {
    let mut events_by_round: IndexMap<i64, Vec<&str>> = IndexMap::new();
    for impact in &ev.events {
        events_by_round.entry(impact.event.round).or_default().push(&impact.event.content);
    }
    let mut lines = Vec::new();
    for s in &ev.stats {
        for text in events_by_round.get(&s.round).map(Vec::as_slice).unwrap_or(&[]) {
            lines.push(format!("  BREAKING NEWS at round {}: {}", s.round, clip(text, 300)));
        }
        let mut actions: Vec<(&String, &i64)> = s.actions.iter().collect();
        actions.sort();
        let mix =
            actions.iter().filter(|(_, v)| **v != 0).map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ");
        lines.push(format!(
            "r{}: {} active, {} new posts, sentiment {}; actions: {}",
            s.round,
            s.active_agents,
            s.new_posts,
            signed(s.sentiment),
            if mix.is_empty() { "none" } else { &mix }
        ));
    }
    lines
}

fn event_lines(ev: &Evidence) -> Vec<String> {
    ev.events
        .iter()
        .map(|e| {
            format!(
                "round {}: \"{}\" -> sentiment {} before vs {} from that round on; posts/round {} before vs {} after",
                e.event.round,
                clip(&e.event.content, 300),
                signed(e.sentiment_before),
                signed(e.sentiment_after),
                py_float(e.posts_before),
                py_float(e.posts_after)
            )
        })
        .collect()
}

/// Every agent's posts' scored sentiments, by author.
pub(crate) fn scores_by_author(ev: &Evidence) -> IndexMap<String, Vec<f64>> {
    let mut scores: IndexMap<String, Vec<f64>> = IndexMap::new();
    for (post_id, score) in &ev.post_sentiment {
        if let Some(post) = ev.posts.get(post_id) {
            scores.entry(post.author_id.clone()).or_default().push(*score);
        }
    }
    scores
}

/// One short line per agent, so the camps can cover everyone, including agents who never posted.
fn roster_lines(ev: &Evidence) -> Vec<String> {
    let scores = scores_by_author(ev);
    let mut agents: Vec<&AgentStats> = ev.agents.iter().collect();
    agents.sort_by(|a, b| a.persona.id.cmp(&b.persona.id));
    agents
        .into_iter()
        .map(|a| {
            let mood = match scores.get(&a.persona.id) {
                Some(values) if !values.is_empty() => format!(
                    "mean post sentiment {}",
                    signed(Some(py_sum(values.iter().copied()) / values.len() as f64))
                ),
                _ => "no scored posts".into(),
            };
            let kind = if a.persona.entity_id.is_some() { "organisation" } else { "person" };
            format!(
                "{} @{} ({kind}): {} posts, {mood}; starting stance: {}",
                a.persona.id,
                a.persona.handle,
                a.posts,
                clip(&a.persona.stance, 90)
            )
        })
        .collect()
}

fn agent_lines(ev: &Evidence, limit: usize) -> Vec<String> {
    ev.agents
        .iter()
        .take(limit)
        .map(|a| {
            format!(
                "@{} ({}, {}): {} followers, {} posts, engagement {}; {}",
                a.persona.handle,
                a.persona.id,
                a.persona.name,
                a.followers,
                a.posts,
                a.engagement,
                clip(&a.persona.bio, 140)
            )
        })
        .collect()
}

fn shift_lines(ev: &Evidence) -> Vec<String> {
    let mut lines = Vec::new();
    for s in &ev.shifts {
        lines.push(format!(
            "@{} ({}), {} posts. Starting stance: {}",
            s.persona.handle,
            s.persona.id,
            s.posts,
            clip(&s.persona.stance, 160)
        ));
        lines.push(format!("    first #{} r{}: \"{}\"", s.first.id, s.first.round, clip(&s.first.content, 200)));
        lines.push(format!("    last  #{} r{}: \"{}\"", s.last.id, s.last.round, clip(&s.last.content, 200)));
        if !s.last_note.is_empty() {
            lines.push(format!("    latest private note: {}", clip(&s.last_note, 160)));
        }
    }
    lines
}

fn thread_lines(ev: &Evidence) -> Vec<String> {
    let mut lines = Vec::new();
    for t in &ev.threads {
        lines.push(format!("Thread ({} replies) {}", t.size, post_line(&t.root, ev)));
        lines.extend(t.replies.iter().map(|r| format!("    {}", post_line(r, ev))));
    }
    lines
}

fn entity_lines(graph: &KnowledgeGraph, limit: usize) -> Vec<String> {
    graph.entities.iter().take(limit).map(|e| format!("{} ({}): {}", e.name, e.kind, clip(&e.summary, 120))).collect()
}

/// Simulated rounds only; round 0 holds the opening posts.
pub fn rounds_done(ev: &Evidence) -> usize {
    ev.stats.iter().filter(|s| s.round > 0).count()
}

/// Every action that is not `nothing`.
pub fn total_actions(ev: &Evidence) -> i64 {
    ev.action_totals.iter().filter(|(k, _)| k.as_str() != ActionType::Nothing.value()).map(|(_, v)| v).sum()
}

pub fn digest_text(ev: &Evidence, graph: Option<&KnowledgeGraph>) -> String {
    let mut totals: Vec<(&String, &i64)> = ev.action_totals.iter().collect();
    totals.sort();
    let totals_text = totals.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ");
    let blocks: Vec<(&str, Vec<String>)> = vec![
        (
            "SCALE",
            vec![
                format!(
                    "{} agents, {} rounds simulated, {} posts, {} actions in total",
                    ev.people.len(),
                    rounds_done(ev),
                    ev.posts.len(),
                    total_actions(ev)
                ),
                format!("action totals: {}", if totals_text.is_empty() { "none" } else { &totals_text }),
            ],
        ),
        ("KEY ENTITIES FROM THE SEED DOCUMENTS", graph.map(|g| entity_lines(g, 15)).unwrap_or_default()),
        ("ROUND TIMELINE (sentiment -1 hostile .. +1 favourable toward the subject)", timeline_lines(ev)),
        ("INJECTED EVENTS AND MEASURED IMPACT", event_lines(ev)),
        (
            "TOP POSTS BY ENGAGEMENT (likes + 2*reposts + replies - dislikes)",
            ev.top_posts.iter().map(|p| post_line(p, ev)).collect(),
        ),
        ("MOST ACTIVE THREADS", thread_lines(ev)),
        ("MOST-FOLLOWED AGENTS", agent_lines(ev, 10)),
        ("EVERY AGENT (place each one in exactly one camp)", roster_lines(ev)),
        ("STANCE TRAJECTORIES (first vs latest post of the most vocal agents)", shift_lines(ev)),
    ];
    blocks
        .into_iter()
        .filter(|(_, lines)| !lines.is_empty())
        .map(|(title, lines)| format!("## {title}\n{}", lines.join("\n")))
        .collect::<Vec<_>>()
        .join("\n\n")
}
