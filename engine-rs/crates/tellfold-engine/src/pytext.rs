//! The bits of Python's string handling the engine depends on and Rust does differently: `re`'s `\w`,
//! `unicodedata.combining`, `str.split()` joins, and code-point slicing.

use tellfold_models::text::{py_isspace, py_split};

use crate::unicode_tables::{COMBINING, WORD};

fn in_table(table: &[(u32, u32)], c: char) -> bool {
    let cp = u32::from(c);
    table
        .binary_search_by(|&(lo, hi)| {
            if hi < cp {
                std::cmp::Ordering::Less
            } else if lo > cp {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// `re.match(r"\w", c)` for a `str` pattern.
pub fn is_word(c: char) -> bool {
    in_table(WORD, c)
}

/// `unicodedata.combining(c) != 0`.
pub fn is_combining(c: char) -> bool {
    in_table(COMBINING, c)
}

/// `re.findall(r"\w{min,}", text)`.
pub fn words(text: &str, min: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start: Option<(usize, usize)> = None; // (byte offset, chars so far)
    for (i, c) in text.char_indices() {
        if is_word(c) {
            match &mut start {
                Some((_, n)) => *n += 1,
                None => start = Some((i, 1)),
            }
        } else if let Some((s, n)) = start.take() {
            if n >= min {
                out.push(&text[s..i]);
            }
        }
    }
    if let Some((s, n)) = start {
        if n >= min {
            out.push(&text[s..]);
        }
    }
    out
}

/// `" ".join(text.split())`.
pub fn squash(text: &str) -> String {
    py_split(text).collect::<Vec<_>>().join(" ")
}

/// `text.rstrip()`.
pub fn rstrip(text: &str) -> &str {
    text.trim_end_matches(py_isspace)
}

/// `len(text)` in code points.
pub fn char_len(text: &str) -> usize {
    text.chars().count()
}
