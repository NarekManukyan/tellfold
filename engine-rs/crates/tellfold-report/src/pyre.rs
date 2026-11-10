//! The report's regular expressions, written so they match what Python's `re` matches: `\w`, `\s` and `\b` are
//! spelled out with Python's own classes (see `tellfold_engine::pytext`), and look-arounds run on `fancy-regex`.

use std::sync::OnceLock;

use fancy_regex::Regex;
use tellfold_engine::pytext::{digit_class, space_class, word_class};

/// Expands `{W}` (the body of Python's `\w` class), `{S}` (`\s`) and `{D}` (`\d`) in a pattern and compiles it.
pub fn py_regex(pattern: &str) -> Regex {
    let expanded = pattern.replace("{W}", word_class()).replace("{S}", space_class()).replace("{D}", digit_class());
    Regex::new(&expanded).unwrap_or_else(|e| panic!("bad pattern {pattern}: {e}"))
}

macro_rules! py_pattern {
    ($name:ident, $pattern:expr) => {
        pub fn $name() -> &'static Regex {
            static RE: OnceLock<Regex> = OnceLock::new();
            RE.get_or_init(|| py_regex($pattern))
        }
    };
}

// `(?<![\w&#])#(\d+)\b`; read the id with `decimal_int`, since `\d` takes digits of every script.
py_pattern!(post_ref, r"(?<![{W}&#])#([{D}]+)(?![{W}])");
// `(?<![\w@./])@([A-Za-z0-9_]{1,40})`
py_pattern!(handle_ref, r"(?<![{W}@./])@([A-Za-z0-9_]{1,40})");
// `(?<![\w&#])([A-Z])#(\d+)\b`: a post in one run of a study, cited as "A#12".
py_pattern!(prefixed_ref, r"(?<![{W}&#])([A-Z])#([{D}]+)(?![{W}])");
// A quoted fragment: straight, curly, German low-high or guillemet quotes, on one line.
py_pattern!(quote, r#""([^"\n]+)"|“([^”\n]+)”|„([^“”\n]+)[“”]|«[{S}]*([^»\n]+?)[{S}]*»"#);
// `[.!?。！？](?=\s|$)|\n`
py_pattern!(sentence_end, r"[.!?。！？](?=[{S}]|$)|\n");
// `\.{3}|…`
py_pattern!(ellipsis, r"\.{3}|…");
// `(`[^`\n]+`)`
py_pattern!(code_span, r"`[^`\n]+`");
// `\*\*(.+?)\*\*`
py_pattern!(bold, r"\*\*(.+?)\*\*");
// `(?<![*\w])\*(?=\S)(.+?)(?<=\S)\*(?![*\w])`
py_pattern!(italic, r"(?<![*{W}])\*(?=[^{S}])(.+?)(?<=[^{S}])\*(?![*{W}])");
// `[-*•]\s+(.*)`, `\d+[.)]\s+(.*)` and `#{1,6}\s+(.*)`, anchored like `re.match`
py_pattern!(bullet, r"^[-*•][{S}]+(.*)");
py_pattern!(numbered, r"^[{D}]+[.)][{S}]+(.*)");
py_pattern!(heading, r"^#{1,6}[{S}]+(.*)");
// `[^\W_]{4,}|@\w+|#\d+` (chat.py)
py_pattern!(chat_word, r"(?:(?![_])[{W}]){4,}|@[{W}]+|#[{D}]+");
