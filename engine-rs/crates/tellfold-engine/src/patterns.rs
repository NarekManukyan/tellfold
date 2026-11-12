//! Compiled-once regular expressions for the modules that read the user's text ([`crate::redact`],
//! [`crate::material`]).

pub(crate) fn re(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).expect("valid pattern")
}

/// `pattern!(name, r"...")` defines `fn name() -> &'static Regex`, compiled on first use.
macro_rules! pattern {
    ($name:ident, $pattern:expr) => {
        fn $name() -> &'static regex::Regex {
            static R: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
            R.get_or_init(|| crate::patterns::re($pattern))
        }
    };
}
