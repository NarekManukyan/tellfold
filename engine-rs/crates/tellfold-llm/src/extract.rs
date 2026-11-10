//! `extract_json`: the first JSON value in a model's reply, fenced or bare (port of `tellfold.llm.extract_json`).
//!
//! The decoder is Python's `json.JSONDecoder(strict=False).raw_decode`: raw line breaks and tabs inside strings are
//! accepted, because models often write long text fields with literal line breaks. A strict decoder would reject
//! the whole object, and the search would fall through to the first nested object instead.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Map, Number, Value};

const MAX_DEPTH: usize = 900; // CPython raises RecursionError somewhat deeper; no model reply nests this far

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("no JSON value found in the reply")]
pub struct NoJson;

fn fence() -> &'static Regex {
    static FENCE: OnceLock<Regex> = OnceLock::new();
    // Python's \s is str.isspace(), which adds U+001C..U+001F to Unicode White_Space
    FENCE.get_or_init(|| Regex::new(r"(?s)```(?:json|JSON)?[\s\x1C-\x1F]*(.*?)```").expect("valid pattern"))
}

/// The first JSON value embedded in `text`, preferring `prefer` (`'{'` or `'['`) as the opening character. Fenced
/// blocks are tried before the whole text.
pub fn extract_json(text: &str, prefer: char) -> Result<Value, NoJson> {
    let mut candidates: Vec<&str> = fence().captures_iter(text).map(|c| c.get(1).map_or("", |m| m.as_str())).collect();
    candidates.push(text);
    let other = if prefer == '{' { '[' } else { '{' };
    for opener in [prefer, other] {
        for chunk in &candidates {
            for (start, _) in chunk.match_indices(opener) {
                if let Some(value) = raw_decode(chunk, start) {
                    return Ok(value);
                }
            }
        }
    }
    Err(NoJson)
}

/// `JSONDecoder(strict=False).raw_decode(s, idx)`: one value starting exactly at `idx`, trailing text ignored.
pub fn raw_decode(s: &str, idx: usize) -> Option<Value> {
    let mut p = Parser { s: s.as_bytes(), text: s, i: idx, depth: 0 };
    p.value()
}

struct Parser<'a> {
    s: &'a [u8],
    text: &'a str,
    i: usize,
    depth: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    /// JSON whitespace only: space, tab, line feed, carriage return.
    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn eat(&mut self, lit: &str) -> bool {
        if self.s[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Option<Value> {
        match self.peek()? {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => self.string().map(Value::String),
            b'n' => self.eat("null").then_some(Value::Null),
            b't' => self.eat("true").then_some(Value::Bool(true)),
            b'f' => self.eat("false").then_some(Value::Bool(false)),
            // Python accepts these; JSON values cannot hold them, so they read as null
            b'N' => self.eat("NaN").then_some(Value::Null),
            b'I' => self.eat("Infinity").then_some(Value::Null),
            b'-' if self.s[self.i..].starts_with(b"-Infinity") => {
                self.i += "-Infinity".len();
                Some(Value::Null)
            }
            b'-' | b'0'..=b'9' => self.number(),
            _ => None,
        }
    }

    fn nested(&mut self) -> Option<()> {
        self.depth += 1;
        (self.depth <= MAX_DEPTH).then_some(())
    }

    fn object(&mut self) -> Option<Value> {
        self.nested()?;
        self.i += 1; // '{'
        let mut map = Map::new();
        self.ws();
        if self.peek()? == b'}' {
            self.i += 1;
            self.depth -= 1;
            return Some(Value::Object(map));
        }
        loop {
            if self.peek()? != b'"' {
                return None;
            }
            let key = self.string()?;
            self.ws();
            if self.peek()? != b':' {
                return None;
            }
            self.i += 1;
            self.ws();
            let value = self.value()?;
            map.insert(key, value); // a repeated key keeps its first position and takes the last value, like dict
            self.ws();
            match self.peek()? {
                b',' => {
                    self.i += 1;
                    self.ws();
                }
                b'}' => {
                    self.i += 1;
                    self.depth -= 1;
                    return Some(Value::Object(map));
                }
                _ => return None,
            }
        }
    }

    fn array(&mut self) -> Option<Value> {
        self.nested()?;
        self.i += 1; // '['
        let mut items = Vec::new();
        self.ws();
        if self.peek()? == b']' {
            self.i += 1;
            self.depth -= 1;
            return Some(Value::Array(items));
        }
        loop {
            items.push(self.value()?);
            self.ws();
            match self.peek()? {
                b',' => {
                    self.i += 1;
                    self.ws();
                }
                b']' => {
                    self.i += 1;
                    self.depth -= 1;
                    return Some(Value::Array(items));
                }
                _ => return None,
            }
        }
    }

    fn hex4(&mut self) -> Option<u32> {
        let digits = self.text.get(self.i..self.i + 4)?;
        if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        self.i += 4;
        u32::from_str_radix(digits, 16).ok()
    }

    /// A string; control characters are allowed raw (strict=False). Lone surrogates become U+FFFD.
    fn string(&mut self) -> Option<String> {
        self.i += 1; // opening quote
        let mut out = String::new();
        loop {
            let start = self.i;
            while let Some(b) = self.peek() {
                if b == b'"' || b == b'\\' {
                    break;
                }
                self.i += 1;
            }
            out.push_str(self.text.get(start..self.i)?);
            match self.peek()? {
                b'"' => {
                    self.i += 1;
                    return Some(out);
                }
                _ => {
                    self.i += 1; // backslash
                    let esc = self.peek()?;
                    self.i += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let first = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&first) && self.s[self.i..].starts_with(b"\\u") {
                                let save = self.i;
                                self.i += 2;
                                match self.hex4() {
                                    Some(second) if (0xDC00..0xE000).contains(&second) => {
                                        0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
                                    }
                                    _ => {
                                        self.i = save;
                                        first
                                    }
                                }
                            } else {
                                first
                            };
                            out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                        }
                        _ => return None,
                    }
                }
            }
        }
    }

    /// `-?(0|[1-9]\d*)(\.\d+)?([eE][-+]?\d+)?`, as Python's scanner reads it: a leading zero ends the number.
    fn number(&mut self) -> Option<Value> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        match self.peek()? {
            b'0' => self.i += 1,
            b'1'..=b'9' => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.i += 1;
                }
            }
            _ => return None,
        }
        let int_end = self.i;
        let mut is_float = false;
        if self.peek() == Some(b'.') && matches!(self.s.get(self.i + 1), Some(b'0'..=b'9')) {
            is_float = true;
            self.i += 1;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.i += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let mut j = self.i + 1;
            if matches!(self.s.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if matches!(self.s.get(j), Some(b'0'..=b'9')) {
                is_float = true;
                while matches!(self.s.get(j), Some(b'0'..=b'9')) {
                    j += 1;
                }
                self.i = j;
            }
        }
        let text = &self.text[start..self.i];
        if is_float {
            let f: f64 = text.parse().ok()?;
            return Some(Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null));
        }
        let digits = &self.text[start..int_end];
        if let Ok(v) = digits.parse::<i64>() {
            return Some(Value::from(v));
        }
        if let Ok(v) = digits.parse::<u64>() {
            return Some(Value::from(v));
        }
        // a Python int beyond 64 bits: the nearest float is the best a JSON value can hold
        digits.parse::<f64>().ok().and_then(Number::from_f64).map(Value::Number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn examples() {
        assert_eq!(extract_json(r#"{"a": 1}"#, '{'), Ok(json!({"a": 1})));
        assert_eq!(extract_json("noise {not json} then {\"a\": 3}", '{'), Ok(json!({"a": 3})));
        assert_eq!(extract_json("{\"a\": 1} [1, 2]", '['), Ok(json!([1, 2])));
        assert_eq!(extract_json("{\"t\": \"a\nb\"}", '{'), Ok(json!({"t": "a\nb"})));
        assert_eq!(extract_json("just words", '{'), Err(NoJson));
    }
}
