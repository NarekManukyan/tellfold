//! The `tellfold` command line (port of `tellfold/cli.py`).

pub mod app;
pub mod args;
pub mod commands;
pub mod help;
pub mod llm;
pub mod mcp;
pub mod paths;
pub mod serve;
pub mod spec;
pub mod term;
pub mod ui;
pub mod view;

/// `tellfold.__version__`.
pub use tellfold_models::VERSION;
