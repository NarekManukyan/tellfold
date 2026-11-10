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

/// `s[-n:]`: the last `n` code points.
pub fn py_suffix(s: &str, n: usize) -> &str {
    let count = s.chars().count();
    if count <= n {
        return s;
    }
    match s.char_indices().nth(count - n) {
        Some((i, _)) => &s[i..],
        None => s,
    }
}

/// `str.splitlines()`: splits on every line boundary Python knows, dropping the separators.
pub fn py_splitlines(s: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let boundary = matches!(
            c,
            '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}'
        );
        if !boundary {
            continue;
        }
        lines.push(&s[start..i]);
        let mut end = i + c.len_utf8();
        if c == '\r' {
            if let Some((j, '\n')) = chars.peek().copied() {
                chars.next();
                end = j + 1;
            }
        }
        start = end;
    }
    if start < s.len() {
        lines.push(&s[start..]);
    }
    lines
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
