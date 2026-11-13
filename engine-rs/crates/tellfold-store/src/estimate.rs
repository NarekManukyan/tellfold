//! Model calls a run makes when nothing needs a retry: `estimate_call_split` from `tellfold/runner.py`. Mirrored in
//! the Mac app (`NewRunRequest.estimatedCallSplit`); the reference table in tests pins all three.

use tellfold_models::json::py_round_int;
use tellfold_models::{MaterialSummary, SimConfig, STAKE_RULES};

/// Chunks per graph extraction call (`tellfold/graph.py`).
pub const CHUNKS_PER_CALL: i64 = 4;
/// Graph extraction calls at most.
pub const MAX_EXTRACT_CALLS: i64 = 16;
pub const MAX_EXTRACT_CHUNKS: i64 = MAX_EXTRACT_CALLS * CHUNKS_PER_CALL;
/// Share of the crowd expected to act in a round. Personas carry an activity of about 0.3 to 0.6 and news rounds
/// boost it, so batching every agent each round overstated a real 20-agent run by more than half.
pub const ESTIMATED_ACTIVE_SHARE: f64 = 0.4;

/// Items of audience material the planner reads per call (`tellfold_engine::material::read_items`). Rust-only.
pub const MATERIAL_ITEMS_PER_CALL: i64 = 40;

/// `math.ceil(a / b)` with Python's float division.
fn ceil_div(a: i64, b: i64) -> i64 {
    (a as f64 / b as f64).ceil() as i64
}

/// (agent, planner) model calls. A run that reuses a cast (`cast_from`, a saved `audience`) makes no graph or
/// persona calls, and reads no audience material.
pub fn estimate_call_split(config: &SimConfig, chunks: i64) -> (i64, i64) {
    let batch = config.batch_size.max(1); // the server and CLI refuse batch sizes below 1
    let reused = config.cast_from.is_some() || config.branched_from.is_some() || config.audience.is_some();
    let branched = config.branched_from.is_some() && config.branch_round.is_some();
    let graph = if reused { 0 } else { ceil_div(chunks.min(MAX_EXTRACT_CHUNKS), CHUNKS_PER_CALL) };
    let personas = if reused { 0 } else { ceil_div(config.num_agents, 10) };
    let material = if reused { 0 } else { material_calls(config) };
    // opening posts + sentiment
    let opening = if branched { 0 } else { ceil_div(py_round_int(0.3 * config.num_agents as f64), batch) + 1 };
    let active = 1.max((config.num_agents as f64 * ESTIMATED_ACTIVE_SHARE).ceil() as i64);
    let per_round = ceil_div(active, batch) + 1; // decisions + sentiment
    let rounds = if branched { config.rounds - config.branch_round.unwrap_or(0) } else { config.rounds };
    let checks = rounds + i64::from(!branched); // the opening posts are checked too
    let crowd = crowd_rules_calls(config.crowd_rules, reused || config.material.is_some(), checks);
    (opening + rounds * per_round, graph + personas + material + 1 + crowd)
}

/// Planner calls the crowd rules add, none before [`STAKE_RULES`]: the stakes plan of a cast written from the
/// announcement (not a reused or grounded one), and at most one fact check of organisation posts for the opening and
/// each round, `checks` in all (5 a run of 7 in the fourth accuracy pass, as some batches hold no organisation post).
/// A report retry is not counted, as no other retry is.
pub fn crowd_rules_calls(crowd_rules: i64, cast_given: bool, checks: i64) -> i64 {
    if crowd_rules < STAKE_RULES {
        return 0;
    }
    i64::from(!cast_given) + checks.max(0)
}

/// Planner calls that read a run's audience material (0 without any): the extra usage grounding costs.
pub fn material_calls(config: &SimConfig) -> i64 {
    config.material.as_ref().map_or(0, material_calls_for)
}

/// Planner calls that read this material.
pub fn material_calls_for(material: &MaterialSummary) -> i64 {
    ceil_div(material.items.max(0), MATERIAL_ITEMS_PER_CALL)
}

/// Model calls for a run, shown so users can judge rate-limit cost.
pub fn estimate_calls(config: &SimConfig, chunks: i64) -> i64 {
    let (agent, planner) = estimate_call_split(config, chunks);
    agent + planner
}

#[cfg(test)]
mod tests {
    use super::*;
    use tellfold_models::MaterialSummary;

    fn with_material(items: i64) -> SimConfig {
        let mut config = SimConfig::new("q", Vec::new());
        config.material = Some(MaterialSummary { items, ..Default::default() });
        config
    }

    #[test]
    fn the_crowd_rules_add_the_stakes_plan_and_a_fact_check_a_round() {
        let mut config = SimConfig::new("q", Vec::new());
        let before = estimate_call_split(&config, 10);
        config.crowd_rules = STAKE_RULES;
        assert_eq!(estimate_call_split(&config, 10), (before.0, before.1 + 1 + config.rounds + 1));
        // a cast that is given (reused here) needs no stakes plan, and is still fact-checked
        config.cast_from = Some("other".into());
        let mut plain_reused = config.clone();
        plain_reused.crowd_rules = 0;
        assert_eq!(estimate_call_split(&config, 10).1, estimate_call_split(&plain_reused, 10).1 + config.rounds + 1);
    }

    #[test]
    fn reading_material_costs_one_planner_call_per_forty_items() {
        let plain = SimConfig::new("q", Vec::new());
        assert_eq!(material_calls(&plain), 0);
        for (items, calls) in [(1, 1), (40, 1), (41, 2), (300, 8)] {
            assert_eq!(material_calls(&with_material(items)), calls, "{items} items");
            let (agent, planner) = estimate_call_split(&with_material(items), 10);
            assert_eq!(
                (agent, planner),
                (estimate_call_split(&plain, 10).0, estimate_call_split(&plain, 10).1 + calls)
            );
        }
        // a run that reuses its cast reads no material
        let mut reused = with_material(300);
        reused.cast_from = Some("other".into());
        assert_eq!(estimate_call_split(&reused, 10), {
            let mut r = plain.clone();
            r.cast_from = Some("other".into());
            estimate_call_split(&r, 10)
        });
    }
}
