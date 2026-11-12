//! The report's direction under the crowd rules: counted from each person's first post, mixed only when the sides
//! are close to even or most people took no side. In the first accuracy pass the model called 15 of 20 runs mixed,
//! mostly by reading where sentiment drifted to, not how people first reacted (docs/ACCURACY.md).

use std::future::Future;

use tellfold_llm::canned::fake_responder;
use tellfold_llm::fake::FakeLlm;
use tellfold_models::{Action, ActionType, KnowledgeGraph, Persona, SimConfig, CROWD_RULES};
use tellfold_report::evidence::collect_evidence;
use tellfold_report::report::{build_report, direction_of, first_reaction, render_markdown, DIRECTION_TASK};
use tellfold_store::world::World;

fn block_on<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}

#[test]
fn a_side_needs_three_in_five_of_the_people_who_took_one() {
    assert_eq!(direction_of(6, 2, 2), "negative");
    assert_eq!(direction_of(3, 2, 0), "negative"); // exactly 60%
    assert_eq!(direction_of(1, 6, 1), "positive");
    assert_eq!(direction_of(5, 4, 1), "mixed"); // 56% is close to even
    assert_eq!(direction_of(4, 4, 0), "mixed");
}

#[test]
fn a_crowd_that_mostly_took_no_side_is_mixed() {
    assert_eq!(direction_of(3, 0, 4), "mixed"); // 3 of 7 took a side
    assert_eq!(direction_of(4, 0, 4), "negative"); // half did, all against
    assert_eq!(direction_of(0, 0, 0), "mixed");
}

/// Four people and an organisation. Ana starts against and comes round; the council's account defends the plan.
fn world(path: &std::path::Path) -> World {
    let w = World::open(path, 0).unwrap();
    let mut council = Persona::new("a01", "city_council", "City Council", "bio", "background", "defends the plan");
    council.entity_id = Some("city-council".into());
    w.add_personas(&[
        council,
        Persona::new("a02", "ana", "Ana", "bio", "background", "critical"),
        Persona::new("a03", "ben", "Ben", "bio", "background", "critical"),
        Persona::new("a04", "cara", "Cara", "bio", "background", "unsure"),
        Persona::new("a05", "dev", "Dev", "bio", "background", "supportive"),
    ])
    .unwrap();
    let post = |round: i64, agent: &str, text: &str| {
        w.apply(round, &Action::new(agent, ActionType::Post).content(text)).unwrap().unwrap()
    };
    let scores = [
        (post(0, "a01", "The new fares fund more buses."), 0.8),
        (post(0, "a02", "15% more for the same bus? No."), -0.7),
        (post(1, "a03", "Another rise. Unfair on shift workers."), -0.6),
        (post(1, "a05", "Fine by me if the buses improve."), 0.5),
        (post(2, "a02", "Okay, the extra routes do help."), 0.6),
        (post(2, "a04", "Not sure what to think yet."), 0.0),
    ];
    w.save_post_sentiment(&scores).unwrap();
    w
}

#[test]
fn the_first_reaction_counts_each_persons_first_post_and_leaves_organisations_out() {
    let tmp = tempfile::tempdir().unwrap();
    let ev = collect_evidence(&world(&tmp.path().join("world.db"))).unwrap();
    let reaction = first_reaction(&ev).unwrap();
    // Ana counts as against (her first post), the council not at all
    assert_eq!((reaction.against, reaction.in_favour, reaction.neither), (2, 1, 1));
    assert_eq!(reaction.direction, "negative");
}

#[test]
fn no_scored_post_means_no_first_reaction() {
    let tmp = tempfile::tempdir().unwrap();
    let w = World::open(&tmp.path().join("world.db"), 0).unwrap();
    w.add_personas(&[Persona::new("a01", "ana", "Ana", "bio", "background", "critical")]).unwrap();
    w.apply(0, &Action::new("a01", ActionType::Post).content("No.")).unwrap();
    assert_eq!(first_reaction(&collect_evidence(&w).unwrap()), None);
}

#[test]
fn the_report_gets_the_direction_only_under_the_rules() {
    for (rules, want) in [(CROWD_RULES, true), (0, false)] {
        let tmp = tempfile::tempdir().unwrap();
        let w = world(&tmp.path().join("world.db"));
        let config = SimConfig { crowd_rules: rules, ..SimConfig::new("How will riders react?", vec![]) };
        let llm = FakeLlm::new(fake_responder);
        let report = block_on(build_report(&llm, &w, &KnowledgeGraph::default(), &config, "sonnet")).unwrap();
        let prompt = llm.calls.lock().unwrap().iter().find(|c| c.0 == "report.write").unwrap().2.clone();
        assert_eq!(prompt.contains(DIRECTION_TASK), want);
        assert_eq!(prompt.contains("- overall direction: negative"), want);
        assert_eq!(report.first_reaction.is_some(), want);
        assert_eq!(render_markdown(&report).contains("**First reaction:** negative"), want);
    }
}
