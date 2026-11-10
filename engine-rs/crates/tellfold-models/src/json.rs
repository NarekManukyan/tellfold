//! JSON text exactly as the Python engine writes it, and CPython's float arithmetic where results are stored.
//!
//! * [`to_string`] / [`to_string_pretty`] match pydantic's `model_dump_json()` / `model_dump_json(indent=2)`.
//!   pydantic-core formats floats with ryu but writes a `+` in positive exponents (`1e+16`).
//! * [`py_dumps`] matches `json.dumps(value, ensure_ascii=False)`: `", "` and `": "` separators and Python's
//!   float repr (`1e-05`).
//! * [`py_round`] is Python's `round(x, n)`, [`py_sum`] the compensated `sum()` of CPython 3.12 and later.

use std::io::{self, Write};

use serde::Serialize;
use serde_json::ser::{CompactFormatter, Formatter, PrettyFormatter};

/// Compact JSON as pydantic's `model_dump_json()` writes it.
pub fn to_string<T: Serialize + ?Sized>(value: &T) -> String {
    write_with(value, Pydantic(CompactFormatter))
}

/// JSON with a two-space indent as pydantic's `model_dump_json(indent=2)` writes it.
pub fn to_string_pretty<T: Serialize + ?Sized>(value: &T) -> String {
    write_with(value, Pydantic(PrettyFormatter::with_indent(b"  ")))
}

/// `json.dumps(value, ensure_ascii=False)`.
pub fn py_dumps<T: Serialize + ?Sized>(value: &T) -> String {
    write_with(value, PyDumps)
}

fn write_with<T: Serialize + ?Sized, F: Formatter>(value: &T, formatter: F) -> String {
    let mut out = Vec::with_capacity(128);
    let mut ser = serde_json::Serializer::with_formatter(&mut out, formatter);
    value.serialize(&mut ser).expect("serialising to memory cannot fail for these types");
    String::from_utf8(out).expect("serde_json writes UTF-8")
}

/// Python's `repr(float)`: the shortest round-tripping digits, in exponent form when the decimal exponent is below
/// -4 or at least 16, with a sign and two or more exponent digits.
pub fn py_float_repr(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    let (negative, digits, exp) = shortest_digits(x);
    let sign = if negative { "-" } else { "" };
    // value = 0.d1d2... * 10^decpt
    let decpt = exp + 1;
    if (-3..=16).contains(&decpt) {
        let body = if decpt <= 0 {
            format!("0.{}{}", "0".repeat((-decpt) as usize), digits)
        } else if decpt as usize >= digits.len() {
            format!("{}{}.0", digits, "0".repeat(decpt as usize - digits.len()))
        } else {
            let (int, frac) = digits.split_at(decpt as usize);
            format!("{int}.{frac}")
        };
        format!("{sign}{body}")
    } else {
        let mantissa = if digits.len() == 1 { digits.clone() } else { format!("{}.{}", &digits[..1], &digits[1..]) };
        let e_sign = if exp < 0 { '-' } else { '+' };
        format!("{sign}{mantissa}e{e_sign}{:02}", exp.abs())
    }
}

/// (negative, significant digits without leading/trailing zeros, decimal exponent of the first digit).
fn shortest_digits(x: f64) -> (bool, String, i32) {
    let text = format!("{:e}", x.abs()); // shortest round-trip digits, e.g. "1.25e-1"
    let (mantissa, exp) = text.split_once('e').expect("LowerExp always has an exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0".to_string() } else { digits.to_string() };
    (x.is_sign_negative(), digits, exp.parse().expect("integer exponent"))
}

/// pydantic-core's float text: ryu, with `e+` for positive exponents; non-finite values as `null`.
fn pydantic_float(x: f64) -> String {
    if !x.is_finite() {
        return "null".into();
    }
    let mut buf = ryu::Buffer::new();
    let text = buf.format_finite(x);
    match text.find('e') {
        Some(i) if text.as_bytes().get(i + 1) != Some(&b'-') => format!("{}e+{}", &text[..i], &text[i + 1..]),
        _ => text.to_string(),
    }
}

/// Python's `round(x, ndigits)`: the correctly rounded decimal (ties to even on the exact binary value).
pub fn py_round(x: f64, ndigits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.ndigits$}").parse().expect("formatted float parses")
}

/// Python's `round(x)` for a float: nearest integer, ties to even.
pub fn py_round_int(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// `sum(values)` over floats in CPython 3.12+: Neumaier compensated summation, as the bundled engine computes it.
pub fn py_sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut total = 0.0f64;
    let mut c = 0.0f64;
    for x in values {
        let t = total + x;
        if total.abs() >= x.abs() {
            c += (total - t) + x;
        } else {
            c += (x - t) + total;
        }
        total = t;
    }
    if c != 0.0 && c.is_finite() {
        total += c;
    }
    total
}

/// Wraps a serde_json formatter, replacing only how floats are written.
struct Pydantic<F>(F);

macro_rules! delegate {
    ($($name:ident),*) => {$(
        fn $name<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> { self.0.$name(w) }
    )*};
}

impl<F: Formatter> Formatter for Pydantic<F> {
    delegate!(begin_array, end_array, begin_object, end_object, end_array_value, end_object_value);

    fn begin_array_value<W: ?Sized + Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        self.0.begin_array_value(w, first)
    }
    fn begin_object_key<W: ?Sized + Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        self.0.begin_object_key(w, first)
    }
    fn end_object_key<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.0.end_object_key(w)
    }
    fn begin_object_value<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.0.begin_object_value(w)
    }
    fn write_f64<W: ?Sized + Write>(&mut self, w: &mut W, value: f64) -> io::Result<()> {
        w.write_all(pydantic_float(value).as_bytes())
    }
    fn write_f32<W: ?Sized + Write>(&mut self, w: &mut W, value: f32) -> io::Result<()> {
        self.write_f64(w, value as f64)
    }
}

/// `json.dumps` separators and float repr. Python writes `NaN`/`Infinity` for non-finite floats.
struct PyDumps;

impl Formatter for PyDumps {
    fn begin_array_value<W: ?Sized + Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            w.write_all(b", ")
        }
    }
    fn begin_object_key<W: ?Sized + Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            w.write_all(b", ")
        }
    }
    fn begin_object_value<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        w.write_all(b": ")
    }
    fn write_f64<W: ?Sized + Write>(&mut self, w: &mut W, value: f64) -> io::Result<()> {
        w.write_all(py_float_repr(value).as_bytes())
    }
    fn write_f32<W: ?Sized + Write>(&mut self, w: &mut W, value: f32) -> io::Result<()> {
        self.write_f64(w, value as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_float_repr() {
        for (x, want) in [
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (123.456, "123.456"),
            (1.5e-7, "1.5e-07"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
        ] {
            assert_eq!(py_float_repr(x), want, "{x}");
        }
    }

    #[test]
    fn dumps_separators() {
        let v = serde_json::json!({"a": [1, 2.5, "é\n"], "b": {}});
        assert_eq!(py_dumps(&v), r#"{"a": [1, 2.5, "é\n"], "b": {}}"#);
        assert_eq!(to_string(&v), r#"{"a":[1,2.5,"é\n"],"b":{}}"#);
    }
}
