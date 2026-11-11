//! Studies beyond the files on disk (port of `tellfold/compare.py`, `agree.py`, `branch.py` and `examples.py`):
//! the cited comparison of a study's runs, the "Check it" agreement across repeats, branching a run from a round,
//! packing a finished run as a built-in example, and `.tellfold` bundles that share a run with someone else.

pub mod agree;
pub mod branch;
pub mod bundle;
pub mod compare;
pub mod examples;

use tellfold_llm::LlmError;
use tellfold_report::ReportError;
use tellfold_store::files::RunError;
use tellfold_store::world::WorldError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error(transparent)]
    World(#[from] WorldError),
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Report(#[from] ReportError),
    /// `RunError` in Python: the run cannot be branched there.
    #[error(transparent)]
    Run(#[from] RunError),
    /// `ValueError` in Python: a comparison of fewer than two runs.
    #[error("{0}")]
    Invalid(String),
    /// `ExampleError` in Python: the run cannot ship as an example.
    #[error("{0}")]
    Example(String),
    /// A bundle that cannot be written or opened: the message says why.
    #[error("{0}")]
    Bundle(String),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
}
