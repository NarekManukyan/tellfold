//! Run folders, `world.db` and studies on disk: the Rust port of `tellfold/world.py`, `tellfold/server/reader.py`,
//! `tellfold/study.py` and the file helpers of `tellfold/runner.py`.
//!
//! Everything here reads and writes the same files as the Python engine 0.3.1, byte for byte where Python's output
//! is deterministic, so runs made by either engine open in the other.

pub mod api;
pub mod estimate;
pub mod files;
pub mod reader;
pub mod study;
pub mod world;

pub use reader::RunReader;
pub use world::{World, WorldError};
