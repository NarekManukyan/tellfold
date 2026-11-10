//! The simulation itself: graph extraction, the cast, the round loop and the run pipeline (port of
//! `tellfold/graph.py`, `personas.py`, `engine.py` and `runner.py`).
//!
//! Every seeded choice goes through [`rng::PyRandom`], CPython's Mersenne Twister, so a seed picks the same agents,
//! follows and opening authors as the Python engine did.

pub mod engine;
pub mod graph;
pub mod personas;
pub mod pytext;
pub mod rng;
pub mod runner;
mod unicode_tables;

/// Implements [`tellfold_llm::JsonModel`] with the schema pydantic generated for the type of the same name
/// (`engine-rs/schemas/<Name>.json`).
#[macro_export]
#[doc(hidden)]
macro_rules! engine_model {
    ($ty:ident) => {
        impl ::tellfold_llm::JsonModel for $ty {
            fn schema() -> &'static ::tellfold_llm::Schema {
                static SCHEMA: ::std::sync::OnceLock<::tellfold_llm::Schema> = ::std::sync::OnceLock::new();
                SCHEMA.get_or_init(|| {
                    ::tellfold_llm::Schema::new::<$ty>(
                        ::tellfold_llm::embedded_schema(stringify!($ty)).expect("schema is embedded"),
                    )
                })
            }
        }
    };
}
