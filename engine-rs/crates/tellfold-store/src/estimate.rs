//! Model calls a run makes when nothing needs a retry: `estimate_call_split` from `tellfold/runner.py`. Mirrored in
//! the Mac app (`NewRunRequest.estimatedCallSplit`); the reference table in tests pins all three.

use tellfold_models::json::py_round_int;
use tellfold_models::SimConfig;

/// Chunks per graph extraction call (`tellfold/graph.py`).
pub const CHUNKS_PER_CALL: i64 = 4;
/// Graph extraction calls at most.
pub const MAX_EXTRACT_CALLS: i64 = 16;
pub const MAX_EXTRACT_CHUNKS: i64 = MAX_EXTRACT_CALLS * CHUNKS_PER_CALL;
/// Share of the crowd expected to act in a round. Personas carry an activity of about 0.3 to 0.6 and news rounds
/// boost it, so batching every agent each round overstated a real 20-agent run by more than half.
pub const ESTIMATED_ACTIVE_SHARE: f64 = 0.4;

/// `math.ceil(a / b)` with Python's float division.
fn ceil_div(a: i64, b: i64) -> i64 {
    (a as f64 / b as f64).ceil() as i64
}

/// (agent, planner) model calls. A run that reuses a cast (`cast_from`) makes no graph or persona calls.
pub fn estimate_call_split(config: &SimConfig, chunks: i64) -> (i64, i64) {
    let batch = config.batch_size.max(1); // the server and CLI refuse batch sizes below 1
    let reused = config.cast_from.is_some() || config.branched_from.is_some();
    let branched = config.branched_from.is_some() && config.branch_round.is_some();
    let graph = if reused { 0 } else { ceil_div(chunks.min(MAX_EXTRACT_CHUNKS), CHUNKS_PER_CALL) };
    let personas = if reused { 0 } else { ceil_div(config.num_agents, 10) };
    // opening posts + sentiment
    let opening = if branched { 0 } else { ceil_div(py_round_int(0.3 * config.num_agents as f64), batch) + 1 };
    let active = 1.max((config.num_agents as f64 * ESTIMATED_ACTIVE_SHARE).ceil() as i64);
    let per_round = ceil_div(active, batch) + 1; // decisions + sentiment
    let rounds = if branched { config.rounds - config.branch_round.unwrap_or(0) } else { config.rounds };
    (opening + rounds * per_round, graph + personas + 1)
}

/// Model calls for a run, shown so users can judge rate-limit cost.
pub fn estimate_calls(config: &SimConfig, chunks: i64) -> i64 {
    let (agent, planner) = estimate_call_split(config, chunks);
    agent + planner
}
