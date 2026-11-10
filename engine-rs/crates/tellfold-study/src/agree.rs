//! The "Check it" study: what held across the runs of a repeat study (same question, people and text, other seeds)
//! (port of `tellfold/agree.py`).
//!
//! The spread of sentiment is measured per round and at the end. Two cheap calls do the rest: `study.match_camps`
//! groups the camps of every run (their names differ from run to run), and `study.agree` sorts the runs'
//! conclusions into the claims that held in every run and the ones that split, with a verdict: consistent, mixed
//! or unstable.

use std::collections::{BTreeMap, HashSet};

use indexmap::IndexMap;
use tellfold_llm::{json, Llm};
use tellfold_models::json::py_round;
use tellfold_models::text::py_strip;
use tellfold_models::{Agreement, AgreementCall, CampMatch, CampMatches, RoundBand};
use tellfold_report::fill;
use tellfold_report::report::confidence_value;
use tellfold_store::files::now_iso;

use crate::compare::{load, Member};
use crate::Error;

pub const MAX_RUNS: usize = 5;
pub const DEFAULT_RUNS: usize = 3;

pub const MATCH_SYSTEM: &str = "\
You group opinion camps from several runs of the same social simulation. Each run named its camps itself, so the \
same group of opinion can carry a different name in each run. Put camps that describe the same position in one \
group; a camp that matches nothing is a group of its own. Never put two camps of the same run in one group. Write \
labels in {language}.";

pub const MATCH_TASK: &str = "\
Camps, one per line, as \"<run letter>: <camp name> (<people>) - <summary>\":
{camps}

Return {\"groups\": [{\"label\", \"members\"}]}. label is a short plain name for the group (2-5 words). members lists \
each matching camp exactly as \"<run letter>: <camp name>\", copied from the lines above.";

pub const AGREE_SYSTEM: &str = "\
You check whether a forecast holds up. The same simulated crowd read the same text several times; only chance \
differed between runs. You get each run's conclusion. Decide which claims every run supports and which only some \
runs support. Be strict: a claim held only if every run supports it. Write in {language}.";

pub const AGREE_TASK: &str = "\
STUDY QUESTION: {question}

Final sentiment per run: {finals} (spread {spread}).

{conclusions}

Return one JSON object:
- held: 1-5 short claims that every run supports.
- split: 0-5 short claims that only some runs support; say which runs, e.g. \"(A and C)\".
- verdict: \"consistent\" when the main conclusion and its direction held in every run, \"mixed\" when the main \
conclusion held but important parts split, \"unstable\" when the runs disagree on the main conclusion.
- summary: one plain sentence for the user saying whether the conclusion held up.";

fn min_max(values: &[f64]) -> Option<(f64, f64)> {
    let first = *values.first()?;
    Some(values.iter().fold((first, first), |(lo, hi), &v| (if v < lo { v } else { lo }, if v > hi { v } else { hi })))
}

/// Per round, the lowest and highest score among the runs that scored it.
pub fn bands(members_rounds: &[Vec<(i64, Option<f64>)>]) -> Vec<RoundBand> {
    let mut by_round: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
    for rounds in members_rounds {
        for (r, score) in rounds {
            let scores = by_round.entry(*r).or_default();
            if let Some(s) = score {
                scores.push(*s);
            }
        }
    }
    by_round
        .into_iter()
        .map(|(round, v)| {
            let range = min_max(&v);
            RoundBand {
                round,
                low: range.map(|(lo, _)| py_round(lo, 3)),
                high: range.map(|(_, hi)| py_round(hi, 3)),
                runs: v.len() as i64,
            }
        })
        .collect()
}

fn plus2(value: Option<f64>) -> String {
    value.map_or("n/a".into(), |f| format!("{f:+.2}"))
}

pub async fn build_agreement(
    llm: &dyn Llm,
    study_id: &str,
    members: &[Member],
    question: &str,
    model: &str,
    language: &str,
) -> Result<Agreement, Error> {
    let loaded = members.iter().map(|m| load(m, "")).collect::<Result<Vec<_>, _>>()?;
    let finals: Vec<Option<f64>> = loaded.iter().map(|x| x.digest.final_sentiment).collect();
    let scored: Vec<f64> = finals.iter().flatten().copied().collect();
    let range = min_max(&scored);
    let spread = range.map(|(lo, hi)| py_round(hi - lo, 3));

    let camp_lines: Vec<String> = loaded
        .iter()
        .flat_map(|x| {
            x.digest.camps.iter().map(|c| format!("{}: {} ({}) - {}", x.member.letter, c.name, c.size, c.summary))
        })
        .collect();
    let mut camps = Vec::new();
    if !camp_lines.is_empty() {
        let prompt = fill(MATCH_TASK, &[("camps", &camp_lines.join("\n"))]);
        let system = fill(MATCH_SYSTEM, &[("language", language)]);
        let matched = json::<CampMatches>(llm, &prompt, &system, model, "study.match_camps").await?;
        let loaded_camps: IndexMap<String, Vec<String>> = loaded
            .iter()
            .map(|x| (x.member.letter.clone(), x.digest.camps.iter().map(|c| c.name.clone()).collect()))
            .collect();
        camps = match_camps(&matched, &loaded_camps);
    }

    let conclusions: Vec<String> = loaded
        .iter()
        .filter_map(|x| {
            let r = x.report.as_ref()?;
            let reasons: String = r.reasons.iter().map(|t| format!("\n- {t}")).collect();
            Some(format!(
                "## Run {}\n{}\n{}\nPrediction: {}\nConfidence: {}{reasons}",
                x.member.letter,
                r.title,
                r.summary,
                r.prediction,
                confidence_value(r.confidence)
            ))
        })
        .collect();
    let finals_text = loaded
        .iter()
        .zip(&finals)
        .map(|(x, f)| format!("{} {}", x.member.letter, plus2(*f)))
        .collect::<Vec<_>>()
        .join(", ");
    let spread_text = spread.map_or("n/a".into(), |s| format!("{s:.2}"));
    let prompt = fill(
        AGREE_TASK,
        &[
            ("question", question),
            ("finals", &finals_text),
            ("spread", &spread_text),
            ("conclusions", &conclusions.join("\n\n")),
        ],
    );
    let system = fill(AGREE_SYSTEM, &[("language", language)]);
    let call = json::<AgreementCall>(llm, &prompt, &system, model, "study.agree").await?;
    let rounds: Vec<Vec<(i64, Option<f64>)>> =
        loaded.iter().map(|x| x.digest.rounds.iter().map(|p| (p.round, p.sentiment)).collect()).collect();
    Ok(Agreement {
        study_id: study_id.into(),
        created_at: now_iso(),
        runs: members.len() as i64,
        final_low: range.map(|(lo, _)| lo),
        final_high: range.map(|(_, hi)| hi),
        spread,
        bands: bands(&rounds),
        camps,
        held: call.held,
        split: call.split,
        verdict: call.verdict,
        summary: call.summary,
    })
}

/// `str.partition(sep)`: (before, after), with an empty `after` when `sep` is missing.
fn partition<'a>(text: &'a str, sep: &str) -> (&'a str, &'a str) {
    text.split_once(sep).unwrap_or((text, ""))
}

/// The model's groups, checked: members must name a real camp of a real run, at most one per run, and every camp
/// the model left out becomes a group of its own. Sorted by how many runs a group appeared in.
pub fn match_camps(matched: &CampMatches, loaded_camps: &IndexMap<String, Vec<String>>) -> Vec<CampMatch> {
    let mut known: IndexMap<(String, String), String> = IndexMap::new();
    for (letter, names) in loaded_camps {
        for name in names {
            known.insert((letter.clone(), py_strip(name).to_lowercase()), name.clone());
        }
    }
    let mut placed: HashSet<(String, String)> = HashSet::new();
    let mut out: Vec<CampMatch> = Vec::new();
    for group in &matched.groups {
        let mut names: Vec<String> = Vec::new();
        let mut letters: HashSet<String> = HashSet::new();
        for raw in &group.members {
            let (letter, name) = partition(raw, ":");
            let key = (py_strip(letter).to_string(), py_strip(name).to_lowercase());
            if let Some(real) = known.get(&key) {
                if !placed.contains(&key) && !letters.contains(&key.0) {
                    names.push(format!("{}: {real}", key.0));
                    letters.insert(key.0.clone());
                    placed.insert(key);
                }
            }
        }
        if !names.is_empty() {
            let label = py_strip(&group.label);
            let label = if label.is_empty() { partition(&names[0], ": ").1.to_string() } else { label.to_string() };
            out.push(CampMatch { label, names, runs: letters.len() as i64 });
        }
    }
    for (key, name) in &known {
        if !placed.contains(key) {
            out.push(CampMatch { label: name.clone(), names: vec![format!("{}: {name}", key.0)], runs: 1 });
        }
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.runs));
    out
}
