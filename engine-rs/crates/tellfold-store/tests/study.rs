//! Port of the studies-on-disk tests in tests/test_study.py and tests/test_check.py.

use std::path::PathBuf;

use tellfold_models::{SimConfig, Study, StudyKind, StudyRun};
use tellfold_store::files::load_cast;
use tellfold_store::study::*;

const QUESTION: &str = "Will buyers accept the subscription?";

fn make_config() -> SimConfig {
    let mut c = SimConfig::new(QUESTION, vec!["examples/mossbook_pricing.md".into()]);
    c.num_agents = 6;
    c.rounds = 2;
    c.batch_size = 3;
    c
}

fn labels(study: &Study) -> Vec<(String, String)> {
    study.runs.iter().map(|r| (r.id.clone(), r.variant.clone())).collect()
}

fn pair(a: &str, b: &str) -> (String, String) {
    (a.into(), b.into())
}

#[test]
fn old_configs_still_load() {
    let config: SimConfig = serde_json::from_str(r#"{"question": "q", "seed_files": []}"#).unwrap();
    assert_eq!((config.cast_from, config.study_id, config.variant), (None, None, None));
    assert_eq!((config.branched_from, config.branch_round), (None, None));
}

#[test]
fn plan_variant_starts_a_study_with_the_source_as_draft_a() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path();
    let (study, label) = plan_variant(runs, "src", &make_config(), None, None, "", None).unwrap();
    assert_eq!(labels(&study), [pair("src", "Draft A")]);
    assert_eq!(label, "Draft B");
    assert!(load_study(runs, &study.id).is_none()); // nothing written until the run exists
    join_study(runs, &study, "new", &label).unwrap();
    let saved = load_study(runs, &study.id).unwrap();
    assert_eq!(labels(&saved), [pair("src", "Draft A"), pair("new", "Draft B")]);

    // a third version made from Draft B joins the same study
    let mut from_b = make_config();
    from_b.study_id = Some(study.id.clone());
    from_b.variant = Some("Draft B".into());
    let (third, label) = plan_variant(runs, "new", &from_b, None, None, "", None).unwrap();
    assert_eq!((third.id.as_str(), label.as_str()), (study.id.as_str(), "Draft C"));
    let mut calm = make_config();
    calm.variant = Some("Calm".into());
    let (third, label) = plan_variant(runs, "src", &calm, Some(&study.id), Some(" Bold "), "", None).unwrap();
    assert_eq!(label, "Bold");
    assert_eq!(third.runs[0].variant, "Draft A");
}

#[test]
fn plan_variant_refuses_unknown_and_repeat_studies() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path();
    let err = plan_variant(runs, "src", &make_config(), Some("missing"), None, "", None).unwrap_err();
    assert_eq!(err.to_string(), "there is no study missing");
    let err = plan_variant(runs, "src", &make_config(), Some("../x"), None, "", None).unwrap_err();
    assert_eq!(err.to_string(), "invalid study id");

    let repeat = Study {
        kind: StudyKind::Repeat,
        runs: vec![StudyRun { id: "src".into(), variant: "Run 1".into() }],
        ..Study::new("check-1", "")
    };
    save_study(runs, &repeat).unwrap();
    let err = plan_variant(runs, "src", &make_config(), Some("check-1"), None, "", None).unwrap_err();
    assert!(err.to_string().starts_with("a Check it study holds only repeats"));
    // a new version of a run inside a check starts its own study and keeps no "Run 2" label
    let mut member = make_config();
    member.study_id = Some("check-1".into());
    member.variant = Some("Run 2".into());
    let (study, label) = plan_variant(runs, "r2", &member, None, None, "", None).unwrap();
    assert_ne!(study.id, "check-1");
    assert_eq!(labels(&study), [pair("r2", "Draft A")]);
    assert_eq!(label, "Draft B");
}

#[test]
fn labels_are_trimmed_and_capped() {
    let tmp = tempfile::tempdir().unwrap();
    let long = "é".repeat(80);
    let (_, label) = plan_variant(tmp.path(), "src", &make_config(), None, Some(&long), "", None).unwrap();
    assert_eq!(label.chars().count(), 60);
    let (study, _) = plan_variant(tmp.path(), "src", &make_config(), None, None, "", Some("Original")).unwrap();
    assert_eq!(study.runs[0].variant, "Original");
}

#[test]
fn studies_are_listed_and_found_by_member() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path();
    for (id, member) in [("b-study", "r2"), ("a-study", "r1")] {
        save_study(
            runs,
            &Study {
                runs: vec![StudyRun { id: member.into(), variant: "Draft A".into() }],
                ..Study::new(id, "2026-01-01T00:00:00Z")
            },
        )
        .unwrap();
    }
    std::fs::create_dir_all(runs.join("studies/broken")).unwrap();
    std::fs::write(runs.join("studies/broken/study.json"), "{").unwrap();
    let ids: Vec<String> = list_studies(runs).into_iter().map(|s| s.id).collect();
    assert_eq!(ids, ["a-study", "b-study"]);
    assert_eq!(study_of(runs, "r2", &make_config()).unwrap().id, "b-study");
    let mut own = make_config();
    own.study_id = Some("a-study".into());
    assert_eq!(study_of(runs, "anything", &own).unwrap().id, "a-study");
    assert!(study_of(runs, "r3", &make_config()).is_none());
    let text = std::fs::read_to_string(runs.join("studies/a-study/study.json")).unwrap();
    assert!(text.starts_with("{\n  \"id\": \"a-study\",\n  \"kind\": \"compare\","));
    assert!(text.ends_with("\"planned\": 0\n}\n"));
}

#[test]
fn study_ids() {
    let tmp = tempfile::tempdir().unwrap();
    for bad in ["", "-x", "a/b", "a..b", ".hidden", &"x".repeat(101)] {
        assert!(study_dir(tmp.path(), bad).is_err(), "{bad:?}");
    }
    assert!(study_dir(tmp.path(), &"x".repeat(100)).is_ok());
    let id = new_study_id(None);
    assert_eq!(id.len(), "20260101-000000-abcd".len());
    assert!(study_dir(tmp.path(), &id).is_ok());
}

#[test]
fn repeat_config_uses_the_next_seed_and_drops_branch_fields() {
    let mut source = make_config();
    source.seed = 11;
    source.branched_from = Some("x".into());
    source.branch_round = Some(1);
    source.variant = Some("Original".into());
    let seeds = vec![PathBuf::from("/runs/new/seeds/a.md")];
    let c = repeat_config(&source, "src", 2, &seeds, "s1", Some("sonnet"));
    assert_eq!((c.seed, c.cast_from.as_deref(), c.study_id.as_deref()), (13, Some("src"), Some("s1")));
    assert_eq!((c.variant.as_deref(), c.branched_from, c.branch_round), (Some("Run 3"), None, None));
    assert_eq!((c.agent_model.as_str(), c.seed_files), ("sonnet", vec!["/runs/new/seeds/a.md".to_string()]));
    assert_eq!(repeat_config(&source, "src", 1, &seeds, "s1", None).agent_model, "haiku");
}

#[test]
fn reusing_a_cast_needs_people() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("20260101-000000-empty");
    std::fs::create_dir_all(&dir).unwrap();
    let err = load_cast(&dir).unwrap_err();
    assert_eq!(err.to_string(), "cannot reuse the people of 20260101-000000-empty: it has no topic map or people yet");
    std::fs::write(dir.join("graph.json"), "{}").unwrap();
    std::fs::write(dir.join("personas.json"), "[]").unwrap();
    assert!(load_cast(&dir).unwrap_err().to_string().ends_with("it has no people"));
}
