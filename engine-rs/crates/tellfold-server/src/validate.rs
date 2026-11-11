//! Request validation in pydantic's lax mode, with its error list (`type`, `loc`, `msg`, `input`, `ctx`), which
//! FastAPI returns as a 422 and the Mac app shows as "field: message".

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::error::ApiError;

/// A request body as FastAPI reads it: missing when empty, `json_invalid` when it is not JSON.
pub fn parse_body(bytes: &[u8], required: bool) -> Result<Option<Value>, ApiError> {
    if bytes.is_empty() {
        if required {
            return Err(ApiError::validation(vec![
                json!({"type": "missing", "loc": ["body"], "msg": "Field required", "input": null}),
            ]));
        }
        return Ok(None);
    }
    match serde_json::from_slice::<Value>(bytes) {
        Ok(value) => Ok(Some(value)),
        Err(e) => {
            let text = String::from_utf8_lossy(bytes);
            let chars: Vec<char> = text.chars().collect();
            let offset: usize =
                text.split('\n').take(e.line().saturating_sub(1)).map(|l| l.chars().count() + 1).sum::<usize>()
                    + e.column().saturating_sub(1);
            // Python reports where the bad token starts
            let mut position = offset.min(chars.len());
            while position > 0 && !" \t\r\n,:[{".contains(chars[position - 1]) {
                position -= 1;
            }
            let message = if e.is_eof() || e.is_syntax() { "Expecting value" } else { "Invalid JSON" };
            Err(ApiError::validation(vec![json!({
                "type": "json_invalid", "loc": ["body", position], "msg": "JSON decode error", "input": {},
                "ctx": {"error": message}
            })]))
        }
    }
}

/// Collects pydantic errors while a body's fields are checked in declaration order.
#[derive(Default)]
pub struct Checker {
    pub errors: Vec<Value>,
}

fn at(loc: &[Value], key: impl Into<Value>) -> Vec<Value> {
    let mut out = loc.to_vec();
    out.push(key.into());
    out
}

impl Checker {
    pub fn new() -> Self {
        Checker::default()
    }

    pub fn error(&mut self, kind: &str, loc: Vec<Value>, msg: String, input: &Value, ctx: Option<Value>) {
        let mut item = json!({"type": kind, "loc": loc, "msg": msg, "input": input});
        if let Some(ctx) = ctx {
            item["ctx"] = ctx;
        }
        self.errors.push(item);
    }

    pub fn finish(self) -> Result<(), ApiError> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(ApiError::validation(self.errors))
        }
    }

    /// The body (or a nested model, when `class` names it) as an object.
    pub fn object<'a>(
        &mut self,
        value: &'a Value,
        loc: &[Value],
        class: Option<&str>,
    ) -> Option<&'a Map<String, Value>> {
        match value {
            Value::Object(map) => Some(map),
            _ => {
                // FastAPI validates in Python mode, where a nested model reads attributes too
                let _ = class;
                self.error(
                    "model_attributes_type",
                    loc.to_vec(),
                    "Input should be a valid dictionary or object to extract fields from".into(),
                    value,
                    None,
                );
                None
            }
        }
    }

    fn missing(&mut self, obj: &Map<String, Value>, loc: Vec<Value>) {
        self.error("missing", loc, "Field required".into(), &Value::Object(obj.clone()), None);
    }

    /// An `int` field: `default` None makes it required. Lax mode takes bools, whole floats and numeric strings.
    pub fn int(
        &mut self,
        obj: &Map<String, Value>,
        key: &str,
        loc: &[Value],
        default: Option<i64>,
        ge: Option<i64>,
        le: Option<i64>,
    ) -> Option<i64> {
        let loc = at(loc, key);
        let Some(value) = obj.get(key) else {
            if default.is_none() {
                self.missing(obj, loc);
            }
            return default;
        };
        let parsed = match value {
            Value::Bool(b) => Ok(*b as i64),
            Value::Number(n) => match (n.as_i64(), n.as_f64()) {
                (Some(i), _) => Ok(i),
                (None, Some(f)) if f.fract() == 0.0 && f.abs() < 9.2e18 => Ok(f as i64),
                (None, Some(_)) => {
                    Err(("int_from_float", "Input should be a valid integer, got a number with a fractional part"))
                }
                _ => Err(("int_type", "Input should be a valid integer")),
            },
            Value::String(s) => parse_py_int(s)
                .ok_or(("int_parsing", "Input should be a valid integer, unable to parse string as an integer")),
            _ => Err(("int_type", "Input should be a valid integer")),
        };
        match parsed {
            Err((kind, msg)) => {
                self.error(kind, loc, msg.into(), value, None);
                None
            }
            Ok(n) => {
                if let Some(low) = ge.filter(|low| n < *low) {
                    self.error(
                        "greater_than_equal",
                        loc,
                        format!("Input should be greater than or equal to {low}"),
                        value,
                        Some(json!({"ge": low})),
                    );
                    return None;
                }
                if let Some(high) = le.filter(|high| n > *high) {
                    self.error(
                        "less_than_equal",
                        loc,
                        format!("Input should be less than or equal to {high}"),
                        value,
                        Some(json!({"le": high})),
                    );
                    return None;
                }
                Some(n)
            }
        }
    }

    /// A `str` field. `required` false makes it `str | None = None` (absent or null gives None).
    #[allow(clippy::too_many_arguments)]
    pub fn string(
        &mut self,
        obj: &Map<String, Value>,
        key: &str,
        loc: &[Value],
        required: bool,
        min_length: Option<usize>,
        max_length: Option<usize>,
        pattern: Option<&Regex>,
    ) -> Option<String> {
        let loc = at(loc, key);
        let value = match obj.get(key) {
            None if required => {
                self.missing(obj, loc);
                return None;
            }
            None | Some(Value::Null) if !required => return None,
            None => return None,
            Some(v) => v,
        };
        let Value::String(text) = value else {
            self.error("string_type", loc, "Input should be a valid string".into(), value, None);
            return None;
        };
        let length = text.chars().count();
        if let Some(min) = min_length.filter(|min| length < *min) {
            let plural = if min == 1 { "" } else { "s" };
            self.error(
                "string_too_short",
                loc,
                format!("String should have at least {min} character{plural}"),
                value,
                Some(json!({"min_length": min})),
            );
            return None;
        }
        if let Some(max) = max_length.filter(|max| length > *max) {
            let plural = if max == 1 { "" } else { "s" };
            self.error(
                "string_too_long",
                loc,
                format!("String should have at most {max} character{plural}"),
                value,
                Some(json!({"max_length": max})),
            );
            return None;
        }
        if let Some(re) = pattern.filter(|re| !re.is_match(text)) {
            self.error(
                "string_pattern_mismatch",
                loc,
                format!("String should match pattern '{}'", re.as_str()),
                value,
                Some(json!({"pattern": re.as_str()})),
            );
            return None;
        }
        Some(text.clone())
    }

    /// A list field with a default of `[]`.
    pub fn list<'a>(
        &mut self,
        obj: &'a Map<String, Value>,
        key: &str,
        loc: &[Value],
        max_length: Option<usize>,
    ) -> Option<&'a Vec<Value>> {
        static EMPTY: Vec<Value> = Vec::new();
        let loc = at(loc, key);
        let Some(value) = obj.get(key) else { return Some(&EMPTY) };
        let Value::Array(items) = value else {
            self.error("list_type", loc, "Input should be a valid list".into(), value, None);
            return None;
        };
        if let Some(max) = max_length.filter(|max| items.len() > *max) {
            let plural = if max == 1 { "" } else { "s" };
            self.error(
                "too_long",
                loc,
                format!("List should have at most {max} item{plural} after validation, not {}", items.len()),
                value,
                Some(json!({"field_type": "List", "max_length": max, "actual_length": items.len()})),
            );
            return None;
        }
        Some(items)
    }
}

/// `int(text)` as pydantic reads a string: surrounding whitespace, a sign, and underscores between digits.
pub fn parse_py_int(text: &str) -> Option<i64> {
    let s = tellfold_models::text::py_strip(text);
    let (negative, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let clean: String = digits.chars().filter(|c| *c != '_').collect();
    if !clean.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: i64 = clean.parse().ok()?;
    Some(if negative { -n } else { n })
}

/// A query parameter `int` with optional bounds (`Query(default, ge=..., le=...)`); the input is the raw text.
pub fn query_int(
    raw: Option<&str>,
    key: &str,
    default: Option<i64>,
    ge: Option<i64>,
    le: Option<i64>,
) -> Result<Option<i64>, ApiError> {
    let Some(text) = raw else { return Ok(default) };
    let mut checker = Checker::new();
    let mut obj = Map::new();
    obj.insert(key.into(), Value::String(text.into()));
    let value = checker.int(&obj, key, &[json!("query")], None, ge, le);
    checker.finish()?;
    Ok(value)
}
