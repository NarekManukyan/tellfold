//! Python `str` behaviour the engine depends on.

/// `str.isspace()` for one character: Unicode White_Space plus the ASCII separators U+001C..U+001F.
pub fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `str.strip()`.
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(py_isspace)
}

/// `str.split()` without arguments.
pub fn py_split(s: &str) -> impl Iterator<Item = &str> {
    s.split(py_isspace).filter(|part| !part.is_empty())
}

/// `s[:n]`: the first `n` code points.
pub fn py_prefix(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_split_prefix() {
        assert_eq!(py_strip("\u{1f} a b\u{a0}\n"), "a b");
        assert_eq!(py_split(" a\u{1c}b  c ").collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(py_prefix("héllo", 2), "hé");
        assert_eq!(py_prefix("hé", 5), "hé");
    }
}
