//! pydantic's lax-mode coercion of JSON input, for the fields models fill in.
//!
//! * int: integers, integral floats, booleans, and strings holding an integer (surrounding whitespace, `+`,
//!   single underscores between digits and an integral decimal like `"3.0"` allowed);
//! * float: numbers, booleans and numeric strings (`"inf"`, `"nan"` too);
//! * bool: `true`/`false`, `0`/`1`, `0.0`/`1.0` and the strings pydantic accepts (`"yes"`, `"off"`, ...).
//!
//! Strings stay strict (`3` is not a `str`), and `null` is refused unless the field is optional, as in pydantic.
//! Use with `#[serde(deserialize_with = "lax::int")]` and friends.

use std::fmt;

use serde::de::{self, Deserializer, Visitor};

/// Parses a Python-style numeric literal: strips whitespace and single underscores between digits.
fn clean_number(text: &str) -> Option<String> {
    let s = text.trim();
    if s.is_empty() || s.starts_with('_') || s.ends_with('_') || s.contains("__") {
        return None;
    }
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'_' && !(bytes[i - 1].is_ascii_digit() && bytes[i + 1].is_ascii_digit()) {
            return None;
        }
    }
    Some(s.replace('_', ""))
}

pub fn parse_int(text: &str) -> Option<i64> {
    let s = clean_number(text)?;
    if let Ok(v) = s.strip_prefix('+').unwrap_or(&s).parse::<i64>() {
        if !s.starts_with("+-") {
            return Some(v);
        }
    }
    let f = parse_float(&s)?;
    int_from_float(f)
}

pub fn parse_float(text: &str) -> Option<f64> {
    let s = clean_number(text)?;
    match s.to_ascii_lowercase().trim_start_matches(['+', '-']) {
        "inf" | "infinity" | "nan" => {
            let negative = s.starts_with('-');
            let v = if s.to_ascii_lowercase().contains("nan") { f64::NAN } else { f64::INFINITY };
            return Some(if negative { -v } else { v });
        }
        _ => {}
    }
    if !s.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E')) {
        return None;
    }
    s.parse().ok()
}

fn int_from_float(f: f64) -> Option<i64> {
    (f.is_finite() && f.fract() == 0.0 && f.abs() < 9.2e18).then_some(f as i64)
}

pub fn parse_bool(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "0" | "off" | "f" | "false" | "n" | "no" => Some(false),
        "1" | "on" | "t" | "true" | "y" | "yes" => Some(true),
        _ => None,
    }
}

struct IntVisitor;

impl<'de> Visitor<'de> for IntVisitor {
    type Value = i64;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a valid integer")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<i64, E> {
        Ok(v as i64)
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<i64, E> {
        Ok(v)
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<i64, E> {
        i64::try_from(v).map_err(|_| E::custom("integer out of range"))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<i64, E> {
        int_from_float(v).ok_or_else(|| E::custom("got a number with a fractional part"))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<i64, E> {
        parse_int(v).ok_or_else(|| E::custom("unable to parse string as an integer"))
    }
}

struct FloatVisitor;

impl<'de> Visitor<'de> for FloatVisitor {
    type Value = f64;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a valid number")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<f64, E> {
        Ok(if v { 1.0 } else { 0.0 })
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<f64, E> {
        Ok(v as f64)
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<f64, E> {
        Ok(v as f64)
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<f64, E> {
        Ok(v)
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<f64, E> {
        parse_float(v).ok_or_else(|| E::custom("unable to parse string as a number"))
    }
}

struct BoolVisitor;

impl<'de> Visitor<'de> for BoolVisitor {
    type Value = bool;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a valid boolean")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<bool, E> {
        Ok(v)
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<bool, E> {
        match v {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(E::custom("input should be a valid boolean")),
        }
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<bool, E> {
        self.visit_i64(v.min(2) as i64)
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<bool, E> {
        if v == 0.0 {
            Ok(false)
        } else if v == 1.0 {
            Ok(true)
        } else {
            Err(E::custom("input should be a valid boolean"))
        }
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<bool, E> {
        parse_bool(v).ok_or_else(|| E::custom("input should be a valid boolean"))
    }
}

/// Wraps a visitor so `null` becomes `None`.
struct Optional<V>(V);

impl<'de, V: Visitor<'de>> Visitor<'de> for Optional<V> {
    type Value = Option<V::Value>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.expecting(f)?;
        f.write_str(" or null")
    }
    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_any(self.0).map(Some)
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
        self.0.visit_bool(v).map(Some)
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
        self.0.visit_i64(v).map(Some)
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
        self.0.visit_u64(v).map(Some)
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
        self.0.visit_f64(v).map(Some)
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        self.0.visit_str(v).map(Some)
    }
}

pub fn int<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    d.deserialize_any(IntVisitor)
}

pub fn opt_int<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    d.deserialize_option(Optional(IntVisitor))
}

pub fn float<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    d.deserialize_any(FloatVisitor)
}

pub fn opt_float<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    d.deserialize_option(Optional(FloatVisitor))
}

pub fn boolean<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    d.deserialize_any(BoolVisitor)
}

/// A list of lax integers (`list[int]`).
pub fn int_list<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<i64>, D::Error> {
    #[derive(serde::Deserialize)]
    struct Item(#[serde(deserialize_with = "int")] i64);
    let items: Vec<Item> = serde::Deserialize::deserialize(d)?;
    Ok(items.into_iter().map(|i| i.0).collect())
}

/// A `dict[str, int]` with lax values, in insertion order.
pub fn int_map<'de, D: Deserializer<'de>>(d: D) -> Result<indexmap::IndexMap<String, i64>, D::Error> {
    #[derive(serde::Deserialize)]
    struct Item(#[serde(deserialize_with = "int")] i64);
    let items: indexmap::IndexMap<String, Item> = serde::Deserialize::deserialize(d)?;
    Ok(items.into_iter().map(|(k, v)| (k, v.0)).collect())
}
