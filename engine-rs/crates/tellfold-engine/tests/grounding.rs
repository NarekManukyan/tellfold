//! A crowd built from the user's own material (Rust-only): camps follow the material's spread, every member of the
//! public is written from quotes of one cluster, and each records where it came from.

use std::collections::HashSet;

use indexmap::IndexMap;
use tellfold_engine::material::{cluster, Classified, Cluster, MaterialItem, QUOTES_PER_PERSONA};
use tellfold_engine::personas::{
    generate_cast, plan_cast, plan_grounded_cast, tidy_cast, CastPlan, Ground, CAMPS, SOURCE_MATERIAL,
};
use tellfold_llm::fake::FakeLlm;
use tellfold_models::{Grounding, KnowledgeGraph, Persona};

const QUESTION: &str = "How will riders react to the ferry fare rise?";

fn item(i: usize, kind: &str) -> MaterialItem {
    MaterialItem {
        id: format!("m{:03}", i + 1),
        kind: kind.into(),
        file: format!("{kind} 1"),
        text: format!("quote {i}"),
    }
}

/// 40 items: 24 critical about price (18 reviews, 6 survey answers), 8 critical about reliability, 8 supportive.
fn material() -> (Vec<MaterialItem>, Vec<Cluster>) {
    let mut items = Vec::new();
    let mut read = Vec::new();
    for i in 0..40 {
        let (kind, camp, theme) = match i {
            0..=17 => ("reviews", "lean_critical", "fares keep going up"),
            18..=23 => ("survey", "lean_critical", "fare too high for families"),
            24..=31 => ("emails", "lean_critical", "late sailings every week"),
            _ => ("comments", "strong_support", "new boats are lovely"),
        };
        items.push(item(i, kind));
        read.push(Classified { index: i, camp: camp.into(), theme: theme.into() });
    }
    (items, cluster(&read))
}

fn run<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(future)
}

#[test]
fn the_material_is_clustered_into_its_real_groups() {
    let (_, clusters) = material();
    let shape: Vec<(&str, usize)> = clusters.iter().map(|c| (c.camp.as_str(), c.items.len())).collect();
    assert_eq!(shape, [("lean_critical", 24), ("lean_critical", 8), ("strong_support", 8)]);
}

#[test]
fn camps_follow_the_spread_of_the_material() {
    let (items, clusters) = material();
    let ground = Ground { items: &items, clusters: &clusters };
    let slots = plan_grounded_cast(&KnowledgeGraph::default(), QUESTION, 20, None, false, ground);
    assert_eq!(slots.len(), 20);
    let camps: Vec<&str> = slots.iter().map(|s| s.camp.as_deref().unwrap()).collect();
    // 32 of 40 items are critical, 8 supportive: 16 and 4 of 20 people; no camp the material does not show
    assert_eq!(camps.iter().filter(|c| **c == "lean_critical").count(), 16);
    assert_eq!(camps.iter().filter(|c| **c == "strong_support").count(), 4);
    // within the critical camp, the price cluster (24 items) gets three people for every one of the reliability one (8)
    let themes: Vec<&str> = slots.iter().map(|s| s.grounding.as_ref().unwrap().theme.as_str()).collect();
    assert_eq!(themes.iter().filter(|t| **t == "fares keep going up").count(), 12);
    assert_eq!(themes.iter().filter(|t| **t == "late sailings every week").count(), 4);
}

#[test]
fn each_person_is_written_from_quotes_of_their_own_cluster() {
    let (items, clusters) = material();
    let ground = Ground { items: &items, clusters: &clusters };
    let slots = plan_grounded_cast(&KnowledgeGraph::default(), QUESTION, 20, None, false, ground);
    let mut used: IndexMap<String, (usize, HashSet<String>)> = IndexMap::new();
    for slot in &slots {
        let g = slot.grounding.as_ref().unwrap();
        let cluster = clusters.iter().find(|c| c.theme == g.theme).unwrap();
        let allowed: HashSet<String> = cluster.items.iter().map(|&i| items[i].text.clone()).collect();
        assert_eq!(g.quotes.len(), QUOTES_PER_PERSONA);
        for quote in &g.quotes {
            assert!(allowed.contains(quote), "{quote} is not from {}", g.theme);
            assert!(slot.brief.contains(&format!("\"{quote}\"")), "{}", slot.brief);
        }
        let entry = used.entry(g.theme.clone()).or_default();
        entry.0 += 1;
        entry.1.extend(g.quotes.iter().cloned());
        assert_eq!(slot.camp.as_deref(), Some(cluster.camp.as_str()));
    }
    // people of one cluster are written from different quotes until its items run out
    for (theme, (people, quotes)) in &used {
        let size = clusters.iter().find(|c| &c.theme == theme).unwrap().items.len();
        assert_eq!(quotes.len(), size.min(people * QUOTES_PER_PERSONA), "{theme}");
    }
    // the source of each person: the price cluster is 18 reviews and 6 survey answers
    let price = slots.iter().find(|s| s.grounding.as_ref().unwrap().theme == "fares keep going up").unwrap();
    let want: IndexMap<String, i64> = [("reviews".to_string(), 18), ("survey".to_string(), 6)].into_iter().collect();
    assert_eq!(price.grounding.as_ref().unwrap().counts, want);
    assert_eq!(tellfold_models::material_counts_text(&want), "18 reviews and 6 survey answers");
}

#[test]
fn the_plan_is_deterministic_and_unread_material_changes_nothing() {
    let (items, clusters) = material();
    let ground = Ground { items: &items, clusters: &clusters };
    let graph = KnowledgeGraph::default();
    assert_eq!(
        plan_grounded_cast(&graph, QUESTION, 12, None, true, ground),
        plan_grounded_cast(&graph, QUESTION, 12, None, true, ground)
    );
    let unread = Ground { items: &items, clusters: &[] };
    assert_eq!(
        plan_grounded_cast(&graph, QUESTION, 12, None, false, unread),
        plan_cast(&graph, QUESTION, 12, None, false)
    );
}

#[test]
fn grounded_personas_record_their_source() {
    let (items, clusters) = material();
    let llm = FakeLlm::new(tellfold_llm::canned::fake_responder);
    let plan = CastPlan { fair: false, ground: Some(Ground { items: &items, clusters: &clusters }) };
    let cast = run(generate_cast(&llm, &KnowledgeGraph::default(), QUESTION, 8, "sonnet", "English", plan)).unwrap();
    assert_eq!(cast.len(), 8);
    for p in &cast {
        assert_eq!(p.source.as_deref(), Some(SOURCE_MATERIAL));
        assert!(CAMPS.contains(&p.camp.as_deref().unwrap()));
        let g = p.grounding.as_ref().unwrap();
        assert!(!g.counts.is_empty() && !g.quotes.is_empty());
    }
    let prompts: Vec<String> = llm.calls.lock().unwrap().iter().map(|(_, _, p)| p.clone()).collect();
    assert!(prompts.iter().any(|p| p.contains("real comments by people like them")));
    // and the persona JSON carries it, for the cast editor, the report and saved audiences
    let text = tellfold_models::json::to_string(&cast);
    assert!(text.contains("\"source\":\"material\"") && text.contains("\"grounding\""), "{text}");
}

#[test]
fn a_saved_or_edited_cast_keeps_its_grounding() {
    let mut p = Persona::new("x", "rider", "Rider", "", "", "");
    p.source = Some("edited".into());
    p.grounding = Some(Grounding {
        theme: "  fares keep going up ".into(),
        counts: [("reviews".to_string(), 14), ("tweets".to_string(), 3), ("survey".to_string(), 0)]
            .into_iter()
            .collect(),
        quotes: vec![" quote one ".into(), "".into(), "x".repeat(2000)],
    });
    let tidy = tidy_cast(vec![p, Persona::new("y", "other", "Other", "", "", "")]);
    let g = tidy[0].grounding.as_ref().unwrap();
    assert_eq!(g.theme, "fares keep going up");
    assert_eq!(g.counts.iter().map(|(k, v)| (k.as_str(), *v)).collect::<Vec<_>>(), [("reviews", 14)]);
    assert_eq!(g.quotes.len(), 2);
    assert_eq!(g.quotes[0], "quote one");
    assert!(g.quotes[1].chars().count() <= 601);
    assert_eq!(tidy[0].source.as_deref(), Some("edited"));
    assert!(tidy[1].grounding.is_none());
}
