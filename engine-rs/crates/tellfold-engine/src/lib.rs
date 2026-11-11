//! The simulation itself: graph extraction, the cast, the round loop and the run pipeline (port of
//! `tellfold/graph.py`, `personas.py`, `engine.py` and `runner.py`).
//!
//! Every seeded choice goes through [`rng::PyRandom`], CPython's Mersenne Twister, so a seed picks the same agents,
//! follows and opening authors as the Python engine did.

pub mod engine;
pub mod graph;
pub mod material;
pub mod personas;
pub mod pytext;
pub mod redact;
pub mod rng;
pub mod runner;
mod unicode_tables;
