//! The cast of a run with the stake rules (`STAKE_RULES`): the public is spread over the groups the announcement
//! affects, with stances drawn from the mix the planner expects, bounded so each side keeps a minority. In the
//! second accuracy pass every cast drew about a third supportive and a third critical from the fixed stance mix, so
//! the first reaction came out mixed in 12 of 19 runs (docs/ACCURACY.md).

use serde_json::json;
use tellfold_engine::personas::{
    bound_mix, camp_of, default_mix, generate_planned_cast, plan_staked_cast, CastPlan, CAMPS, MIN_MIDDLE, MIN_SIDE,
    SOURCE_CAST, STANCE_MIX,
};
use tellfold_llm::canned::fake_responder;
use tellfold_llm::fake::{FakeLlm, FakeReply};
use tellfold_llm::Schema;
use tellfold_models::{CastStakes, Entity, KnowledgeGraph, Stake, StanceMix};

const QUESTION: &str = "How will riders react to the 15% bus fare rise?";

fn graph() -> KnowledgeGraph {
    KnowledgeGraph {
        entities: vec![Entity {
            id: "city-council".into(),
            name: "City Council".into(),
            kind: "organization".into(),
            summary: "Raises adult bus fares by 15% from January.".into(),
            source_chunk_ids: vec![0],
        }],
        ..Default::default()
    }
}

fn mix(supportive: f64, critical: f64, undecided: f64, indifferent: f64) -> StanceMix {
    StanceMix { supportive, critical, undecided, indifferent }
}

fn close(a: &StanceMix, b: &StanceMix) -> bool {
    let d = |x: f64, y: f64| (x - y).abs() < 1e-9;
    d(a.supportive, b.supportive)
        && d(a.critical, b.critical)
        && d(a.undecided, b.undecided)
        && d(a.indifferent, b.indifferent)
}

fn groups() -> Vec<Stake> {
    vec![
        Stake { group: "night-shift nurses".into(), stake: "pay 15% more for the only bus home".into(), share: 0.6 },
        Stake { group: "students".into(), stake: "keep their discounted pass".into(), share: 0.4 },
    ]
}

fn stakes(m: StanceMix) -> CastStakes {
    CastStakes { groups: groups(), reason: "most riders pay more".into(), planned: m, mix: bound_mix(&m) }
}

fn responder(purpose: &str, prompt: &str, schema: Option<&Schema>) -> FakeReply {
    if purpose == "personas.stakes" {
        let groups: Vec<_> =
            groups().iter().map(|s| json!({"group": s.group, "stake": s.stake, "share": s.share})).collect();
        let mix = json!({"supportive": 0.05, "critical": 0.8, "undecided": 0.1, "indifferent": 0.05});
        return FakeReply::Json(json!({"groups": groups, "mix": mix, "reason": "most riders pay more"}));
    }
    fake_responder(purpose, prompt, schema)
}

fn run<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(future)
}

#[test]
fn the_bounds_keep_a_minority_on_each_side_and_a_middle() {
    // all against: the other side and the middle are raised to their floors, from the critical share
    let bounded = bound_mix(&mix(0.0, 0.9, 0.1, 0.0));
    assert!(close(&bounded, &mix(MIN_SIDE, 1.0 - MIN_SIDE - MIN_MIDDLE, MIN_MIDDLE, 0.0)), "{bounded:?}");
    // a mix inside the bounds is kept
    assert!(close(&bound_mix(&mix(0.2, 0.5, 0.2, 0.1)), &mix(0.2, 0.5, 0.2, 0.1)));
    // shares are scaled to add up to 1; nonsense counts as nothing; nothing at all is the old fixed mix
    assert!(close(&bound_mix(&mix(2.0, 2.0, 1.0, f64::NAN)), &mix(0.4, 0.4, 0.2, 0.0)));
    assert!(close(&bound_mix(&mix(0.0, 0.0, -1.0, 0.0)), &default_mix()));
}

#[test]
fn the_bounded_mix_always_adds_up_and_respects_the_floors() {
    for m in [mix(1.0, 0.0, 0.0, 0.0), mix(0.0, 0.0, 0.0, 1.0), mix(0.3, 0.3, 0.0, 0.4), mix(0.1, 0.1, 0.7, 0.1)] {
        let b = bound_mix(&m);
        assert!((b.supportive + b.critical + b.undecided + b.indifferent - 1.0).abs() < 1e-9, "{b:?}");
        assert!(b.supportive >= MIN_SIDE - 1e-9 && b.critical >= MIN_SIDE - 1e-9, "{b:?}");
        assert!(b.undecided + b.indifferent >= MIN_MIDDLE - 1e-9, "{b:?}");
    }
}

#[test]
fn the_public_is_spread_over_the_stakes_with_the_planned_stances() {
    let slots = plan_staked_cast(
        &KnowledgeGraph::default(),
        QUESTION,
        10,
        Some(&[]),
        false,
        &stakes(mix(0.15, 0.7, 0.1, 0.05)),
    );
    assert_eq!(slots.len(), 10);
    let count = |text: &str| slots.iter().filter(|s| s.brief.contains(text)).count();
    // shares 0.6 and 0.4 of ten people
    assert_eq!((count("night-shift nurses"), count("students")), (6, 4));
    assert!(slots.iter().all(|s| s.brief.contains("what the announcement changes for them")));
    let camps: Vec<&str> = slots.iter().filter_map(|s| s.camp.as_deref()).collect();
    let side = |c: &str| camps.iter().filter(|k| k.ends_with(c)).count();
    assert_eq!((side("_support"), side("_critical")), (2, 7), "{camps:?}");
    assert!(slots.iter().all(|s| s.camp.as_deref().and_then(|c| CAMPS.iter().position(|k| *k == c)).is_some()));
    assert!(slots
        .iter()
        .all(|s| STANCE_MIX.iter().any(|(st, _)| s.brief.contains(st) && camp_of(st) == s.camp.as_deref())));
}

#[test]
fn entity_voices_are_planned_as_before() {
    let voices = graph().entities;
    let slots = plan_staked_cast(&graph(), QUESTION, 10, Some(&voices), false, &stakes(default_mix()));
    let entity: Vec<_> = slots.iter().filter(|s| s.entity_id.is_some()).collect();
    assert_eq!(entity.len(), 1);
    assert!(entity[0].brief.starts_with("ENTITY VOICE for City Council"));
    assert_eq!(slots.len(), 10);
}

#[test]
fn a_planned_cast_records_the_groups_the_mix_and_its_reason() {
    let llm = FakeLlm::new(responder);
    let plan = CastPlan { fair: true, stakes: true, ..Default::default() };
    let (cast, planned) = run(generate_planned_cast(&llm, &graph(), QUESTION, 10, "sonnet", "English", plan)).unwrap();
    assert_eq!(cast.len(), 10);
    let planned = planned.expect("the planner answered");
    assert_eq!(planned.reason, "most riders pay more");
    assert!(close(&planned.planned, &mix(0.05, 0.8, 0.1, 0.05)));
    // supportive raised to the floor, taken from the critical share
    assert!(close(&planned.mix, &mix(MIN_SIDE, 0.7, 0.1, 0.05)), "{:?}", planned.mix);
    let calls = llm.calls.lock().unwrap().clone();
    assert_eq!(calls.iter().filter(|c| c.0 == "personas.stakes").count(), 1);
    assert!(calls.iter().filter(|c| c.0 == "personas.batch").any(|c| c.2.contains("night-shift nurses")));
    for p in cast.iter().filter(|p| p.entity_id.is_none()) {
        assert!(p.camp.as_deref().is_some_and(|c| CAMPS.contains(&c)), "{p:?}");
        assert_eq!(p.source.as_deref(), Some(SOURCE_CAST));
    }
}

#[test]
fn without_stakes_or_when_the_plan_fails_the_cast_keeps_the_stance_mix() {
    for (stakes, reply) in [(false, true), (true, false)] {
        let llm = FakeLlm::new(move |purpose: &str, prompt: &str, schema: Option<&Schema>| {
            if purpose == "personas.stakes" && !reply {
                return FakeReply::Json(json!({"groups": [], "mix": null, "reason": ""}));
            }
            responder(purpose, prompt, schema)
        });
        let plan = CastPlan { stakes, ..Default::default() };
        let (_, planned) = run(generate_planned_cast(&llm, &graph(), QUESTION, 10, "sonnet", "English", plan)).unwrap();
        assert!(planned.is_none());
        let calls = llm.calls.lock().unwrap().clone();
        assert_eq!(calls.iter().any(|c| c.0 == "personas.stakes"), stakes);
        let batch = calls.iter().find(|c| c.0 == "personas.batch").unwrap();
        assert!(!batch.2.contains("what the announcement changes for them"));
    }
}
