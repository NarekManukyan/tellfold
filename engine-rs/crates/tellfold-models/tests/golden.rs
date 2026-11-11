//! Parity with the Python engine: fixtures written by `engine-rs/golden/make.py`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tellfold_models::{json, lax};

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../golden/fixtures").join(name);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap()
}

#[test]
fn floats_are_written_like_pydantic_and_json_dumps() {
    let f = fixture("numbers.json");
    let values: Vec<f64> =
        f["values"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().parse().unwrap()).collect();
    assert_eq!(json::to_string(&values), f["pydantic"].as_str().unwrap());
    assert_eq!(json::py_dumps(&values), f["python"].as_str().unwrap());
    for (v, text) in values.iter().zip(f["values"].as_array().unwrap()) {
        assert_eq!(json::py_float_repr(*v), text.as_str().unwrap());
    }
}

#[test]
fn sum_and_round_match_cpython() {
    let f = fixture("numbers.json");
    for case in f["sums"].as_array().unwrap() {
        let xs: Vec<f64> =
            case["values"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().parse().unwrap()).collect();
        let total = json::py_sum(xs.iter().copied());
        assert_eq!(json::py_float_repr(total), case["sum"].as_str().unwrap(), "{xs:?}");
        if case["mean3"] != "0.0" || xs.len() != 3 {
            let mean = json::py_round(total / xs.len() as f64, 3);
            assert_eq!(json::py_float_repr(mean), case["mean3"].as_str().unwrap(), "{xs:?}");
        }
    }
    for case in f["rounds"].as_array().unwrap() {
        let x: f64 = case["x"].as_str().unwrap().parse().unwrap();
        let n = case["n"].as_u64().unwrap() as usize;
        assert_eq!(json::py_float_repr(json::py_round(x, n)), case["r"].as_str().unwrap(), "round({x}, {n})");
    }
    for (x, want) in f["ties_even"].as_object().unwrap() {
        assert_eq!(json::py_round_int(x.parse().unwrap()), want.as_i64().unwrap(), "round({x})");
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct M {
    #[serde(default, deserialize_with = "lax::int")]
    i: i64,
    #[serde(default, deserialize_with = "lax::float")]
    f: f64,
    #[serde(default, deserialize_with = "lax::boolean")]
    b: bool,
    #[serde(default)]
    s: String,
    #[serde(default, deserialize_with = "lax::opt_int")]
    o: Option<i64>,
}

#[test]
fn lax_coercion_matches_pydantic() {
    for case in fixture("lax.json").as_array().unwrap() {
        let input = case["input"].as_str().unwrap();
        let got = serde_json::from_str::<M>(input).ok().map(|m| serde_json::to_value(m).unwrap());
        let want = (!case["output"].is_null()).then(|| case["output"].clone());
        assert_eq!(got, want, "{input}");
    }
}
