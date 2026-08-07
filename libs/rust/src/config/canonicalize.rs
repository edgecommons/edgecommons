//! # Configuration — numeric canonicalization
//!
//! **One-liner purpose**: One shared rule for "this JSON number encodes an integer",
//! applied as a pure pass over the raw configuration document at the intake boundary
//! and reused by every integer-typed config field in the library.
//!
//! ## Overview
//! Configuration stores do not agree on how they encode a JSON number. A ConfigMap
//! or a FILE document carries the literal `5000`; the Greengrass config store
//! round-trips every number through a Java `double` and hands back `5000.0`. `serde`
//! will not coerce a float into a `u64`, so the same logical configuration parsed on
//! one platform and failed on another.
//!
//! [`canonicalize_json_numbers`] removes that difference once, at the boundary: every
//! JSON number whose value is an integer is rewritten as an integer JSON number, so
//! everything downstream — schema validation, candidate validators, the typed
//! [`crate::config::model::Config`] snapshot, `component.*` subtrees handed to
//! component code, envelope `tags`, and the published effective configuration — sees
//! one canonical document whatever the store did to it.
//!
//! ## Semantics & Architecture
//! A number backed by a float `f` becomes an integer **iff** `f` is finite,
//! `f.fract() == 0.0`, and the integer candidate round-trips exactly:
//!
//! - `f >= 0.0` and `f < 2^64`: `u = f as u64` with `(u as f64) == f` → unsigned `u`.
//! - `f < 0.0`: `i = f as i64` with `(i as f64) == f` → signed `i`.
//!
//! Anything else is left byte-identical: a fractional value, a value outside the
//! exactly-representable 64-bit window, and every string, boolean, and `null`. There
//! is deliberately **no string or boolean coercion** — coercing `"1.0"` would corrupt
//! legitimately-string fields, and no store this library targets stringifies numbers.
//!
//! The pass is pure and **idempotent**: a canonical document is a fixed point, so
//! applying it at both the pipeline intake and [`crate::config::model::Config::from_value`]
//! is harmless defense in depth. Object keys are never touched.
//!
//! The same rule backs the `serde` helpers in this module, which are the single
//! implementation every integer-typed field in the library's own config sections
//! uses ([`crate::config::model`], [`crate::credentials::config`],
//! [`crate::parameters::config`]). They accept an integer or an integral float and
//! **reject** a fractional value, a negative into an unsigned field, an out-of-range
//! value, and a non-number — loudly, with a message naming the offending value.
//! Silently truncating or saturating a configured value is worse than failing.
//!
//! ## Usage Example
//! ```
//! use edgecommons::config::canonicalize_json_numbers;
//! use serde_json::json;
//!
//! let mut doc = json!({ "component": { "instances": [ { "pollIntervalMs": 5000.0 } ] } });
//! canonicalize_json_numbers(&mut doc);
//! assert_eq!(doc["component"]["instances"][0]["pollIntervalMs"], json!(5000));
//! ```
//!
//! ## Design Choices
//! Read-side and unconditional: nothing rewrites a configuration store. A store
//! already holding doubles (the Greengrass store is cumulative — a redeploy of the
//! original recipe does not clear it) parses correctly on the very next start or
//! configuration update, with no reset.
//!
//! ## Safety & Panics
//! None. The pass recurses over the document; documents come from `serde_json`, whose
//! parser enforces its own nesting limit.
//!
//! ## Related Modules
//! - [`super::model`], [`super::layered`], [`super::validation`].

use serde::{Deserialize, Deserializer};
use serde_json::{Number, Value};

/// `2^64` as an `f64` — the exclusive upper bound of the unsigned window.
///
/// `u64::MAX as f64` rounds *up* to exactly this value, so a naive
/// `(f as u64) as f64 == f` round-trip check would accept `2^64` and silently store
/// `u64::MAX`. The explicit bound is what makes the round-trip honest.
const U64_UPPER_EXCLUSIVE: f64 = 18_446_744_073_709_551_616.0;

/// Rewrites every JSON number in `value` that encodes an integer as an integer number.
///
/// Recurses through objects and arrays; keys, strings, booleans, and `null` are never
/// touched. See the [module docs](self) for the exact rule. The pass is pure and
/// idempotent.
///
/// Component code that obtains raw configuration JSON outside a
/// [`crate::config::model::Config`] snapshot can apply the identical rule with this
/// function; everything reached through `Config` is canonical already.
pub fn canonicalize_json_numbers(value: &mut Value) {
    match value {
        Value::Number(number) => {
            if let Some(canonical) = canonical_integer(number) {
                *number = canonical;
            }
        }
        Value::Array(items) => {
            for item in items {
                canonicalize_json_numbers(item);
            }
        }
        Value::Object(map) => {
            for (_key, entry) in map.iter_mut() {
                canonicalize_json_numbers(entry);
            }
        }
        Value::Null | Value::Bool(_) | Value::String(_) => {}
    }
}

/// The integer form of `number`, or `None` when it is already an integer or is not
/// an exactly-representable integral value.
fn canonical_integer(number: &Number) -> Option<Number> {
    if !number.is_f64() {
        // Already an integer JSON number (`u64`/`i64`-backed) — a fixed point.
        return None;
    }
    integral_number(number.as_f64()?)
}

/// The exact integer `Number` for `f`, or `None` when `f` is not an integral value
/// inside the exactly-representable 64-bit window.
fn integral_number(f: f64) -> Option<Number> {
    if !f.is_finite() || f.fract() != 0.0 {
        return None;
    }
    if f >= 0.0 {
        if f >= U64_UPPER_EXCLUSIVE {
            return None;
        }
        let candidate = f as u64;
        // Redundant with the bound above for every finite input, kept because the
        // round-trip *is* the contract stated in the design (D-NC1).
        return ((candidate as f64) == f).then(|| Number::from(candidate));
    }
    let candidate = f as i64;
    ((candidate as f64) == f).then(|| Number::from(candidate))
}

/// Reads `value` as a `u64`, accepting an integer or an integral JSON float.
///
/// Returns a message naming the offending value for a fractional number, a negative
/// number, a value outside the exactly-representable 64-bit window, and a non-number.
/// Never truncates and never saturates.
fn integral_u64(value: &Value) -> std::result::Result<u64, String> {
    let Value::Number(number) = value else {
        return Err(format!("expected an integer, got {value}"));
    };
    if let Some(unsigned) = number.as_u64() {
        return Ok(unsigned);
    }
    if let Some(signed) = number.as_i64() {
        return Err(format!("expected a non-negative integer, got {signed}"));
    }
    let float = number
        .as_f64()
        .ok_or_else(|| format!("expected an integer, got {value}"))?;
    if float.fract() != 0.0 {
        return Err(format!(
            "expected an integer, got the fractional number {float}"
        ));
    }
    if float < 0.0 {
        return Err(format!("expected a non-negative integer, got {float}"));
    }
    match integral_number(float) {
        Some(number) => number
            .as_u64()
            .ok_or_else(|| format!("expected an integer within the 64-bit range, got {float}")),
        None => Err(format!(
            "expected an integer within the 64-bit range, got {float}"
        )),
    }
}

/// Reads `value` as a `u64` for a probe that has no error channel: `None` for a
/// fractional, negative, out-of-range, or non-numeric value.
///
/// Used by the free-form `metricEmission.targetConfig` accessors, whose documented
/// contract is to fall back to their schema default rather than to fail. It shares
/// the rule above, so it never truncates `5.5` to `5` or saturates `-5.0` to `0`.
pub(crate) fn as_integral_u64(value: &Value) -> Option<u64> {
    integral_u64(value).ok()
}

/// `serde` deserializer for a required (or defaulted) `u64` config field.
///
/// Accepts an integer or an integral JSON float; rejects everything else with a
/// message naming the value. See the [module docs](self).
pub(crate) fn de_integral_u64<'de, D>(deserializer: D) -> std::result::Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    integral_u64(&value).map_err(serde::de::Error::custom)
}

/// `serde` deserializer for an optional `u64` config field. Absent or `null` yields
/// `None`; a present-but-invalid value is an error, never a silent `None`.
pub(crate) fn de_integral_opt_u64<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<Value>::deserialize(deserializer)? {
        None | Some(Value::Null) => Ok(None),
        Some(value) => integral_u64(&value)
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

/// `serde` deserializer for a required (or defaulted) `usize` config field, on the
/// same rule as [`de_integral_u64`].
#[cfg(feature = "credentials")]
pub(crate) fn de_integral_usize<'de, D>(deserializer: D) -> std::result::Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    let value = de_integral_u64(deserializer)?;
    usize::try_from(value).map_err(|_| {
        serde::de::Error::custom(format!(
            "expected an integer within this platform's usize range, got {value}"
        ))
    })
}

/// `serde` deserializer for an optional `f64` config field that accepts any JSON
/// number (integral or fractional). Absent or `null` yields `None`; a non-number is
/// an error, never a silent `None`.
pub(crate) fn de_opt_f64<'de, D>(deserializer: D) -> std::result::Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<Value>::deserialize(deserializer)? {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number.as_f64().map(Some).ok_or_else(|| {
            serde::de::Error::custom(format!("expected a number, got {}", Value::Number(number)))
        }),
        Some(other) => Err(serde::de::Error::custom(format!(
            "expected a number, got {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn canonical(value: Value) -> Value {
        let mut value = value;
        canonicalize_json_numbers(&mut value);
        value
    }

    // ---------------------------------------------------------------------------------------
    // The §3 semantics table.
    // ---------------------------------------------------------------------------------------

    #[test]
    fn integral_doubles_become_integers() {
        assert_eq!(canonical(json!(5000.0)), json!(5000));
        assert_eq!(canonical(json!(0.0)), json!(0));
        assert_eq!(canonical(json!(1.0)), json!(1));
    }

    #[test]
    fn integers_are_a_fixed_point() {
        assert_eq!(canonical(json!(5000)), json!(5000));
        assert_eq!(canonical(json!(-5)), json!(-5));
        assert_eq!(canonical(json!(u64::MAX)), json!(u64::MAX));
        assert_eq!(canonical(json!(i64::MIN)), json!(i64::MIN));
    }

    #[test]
    fn a_fractional_value_is_left_untouched() {
        assert_eq!(canonical(json!(5000.5)), json!(5000.5));
        assert_eq!(canonical(json!(0.1)), json!(0.1));
        assert_eq!(canonical(json!(-2.5)), json!(-2.5));
    }

    #[test]
    fn a_negative_integral_double_becomes_a_signed_integer() {
        let value = canonical(json!(-5.0));
        assert_eq!(value, json!(-5));
        assert_eq!(value.as_i64(), Some(-5));
        assert!(value.as_u64().is_none(), "a negative never becomes u64");
    }

    #[test]
    fn negative_zero_becomes_zero() {
        let value = canonical(json!(-0.0));
        assert_eq!(value, json!(0));
        assert_eq!(value.as_u64(), Some(0));
    }

    #[test]
    fn large_integral_doubles_inside_the_u64_window_convert() {
        // 1e19 < 2^64 and is exactly representable.
        assert_eq!(
            canonical(json!(1e19)),
            json!(10_000_000_000_000_000_000_u64)
        );
        // 2^63 exactly.
        assert_eq!(
            canonical(json!(9_223_372_036_854_775_808.0_f64)),
            json!(9_223_372_036_854_775_808_u64)
        );
    }

    #[test]
    fn values_outside_the_sixty_four_bit_window_stay_floats() {
        // 1e20 > u64::MAX.
        assert_eq!(canonical(json!(1e20)), json!(1e20));
        assert!(canonical(json!(1e20)).as_u64().is_none());
        // Exactly 2^64: `u64::MAX as f64` rounds up to this, so a naive round-trip
        // check would wrongly accept it.
        assert_eq!(
            canonical(json!(U64_UPPER_EXCLUSIVE)),
            json!(U64_UPPER_EXCLUSIVE)
        );
        assert!(canonical(json!(U64_UPPER_EXCLUSIVE)).as_u64().is_none());
        // Below the negative window (-2^64).
        assert_eq!(
            canonical(json!(-U64_UPPER_EXCLUSIVE)),
            json!(-U64_UPPER_EXCLUSIVE)
        );
        assert!(canonical(json!(-U64_UPPER_EXCLUSIVE)).as_i64().is_none());
    }

    #[test]
    fn the_largest_representable_u64_window_double_converts() {
        // 2^64 - 2048 is the largest double strictly below 2^64.
        let largest = U64_UPPER_EXCLUSIVE - 2048.0;
        let value = canonical(json!(largest));
        assert_eq!(value.as_u64(), Some(18_446_744_073_709_549_568));
        assert!(value.is_u64(), "converted to an integer JSON number");
    }

    #[test]
    fn the_two_to_the_fifty_third_boundary_is_exact() {
        assert_eq!(
            canonical(json!(9_007_199_254_740_992.0_f64)),
            json!(9_007_199_254_740_992_u64)
        );
        // 2^53 + 1 can only arrive as an integer literal; it stays untouched.
        assert_eq!(
            canonical(json!(9_007_199_254_740_993_u64)),
            json!(9_007_199_254_740_993_u64)
        );
    }

    #[test]
    fn the_negative_window_bound_converts_exactly() {
        assert_eq!(
            canonical(json!(-9_223_372_036_854_775_808.0_f64)),
            json!(i64::MIN)
        );
    }

    #[test]
    fn strings_booleans_and_null_are_never_coerced() {
        assert_eq!(canonical(json!("5000")), json!("5000"));
        assert_eq!(canonical(json!("5000.0")), json!("5000.0"));
        assert_eq!(canonical(json!(true)), json!(true));
        assert_eq!(canonical(json!(null)), json!(null));
    }

    // ---------------------------------------------------------------------------------------
    // Structure.
    // ---------------------------------------------------------------------------------------

    #[test]
    fn nested_objects_and_arrays_are_walked() {
        let value = canonical(json!({
            "a": { "b": [ 1.0, { "c": 2.0 }, [ 3.0 ] ] },
            "d": 4.0
        }));
        assert_eq!(
            value,
            json!({ "a": { "b": [ 1, { "c": 2 }, [ 3 ] ] }, "d": 4 })
        );
    }

    #[test]
    fn keys_are_never_touched() {
        let value = canonical(json!({ "5000.0": 1.0, "": 2.0 }));
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(keys, vec!["", "5000.0"]);
    }

    #[test]
    fn the_pass_is_idempotent() {
        let source = json!({
            "ints": [5000.0, -5.0, 5000.5, 1e20, "5000", true, null],
            "nested": { "deep": { "v": 30.0 } }
        });
        let once = canonical(source.clone());
        let twice = canonical(once.clone());
        assert_eq!(once, twice);
    }

    #[test]
    fn an_empty_document_is_unchanged() {
        assert_eq!(canonical(json!({})), json!({}));
        assert_eq!(canonical(json!([])), json!([]));
    }

    // ---------------------------------------------------------------------------------------
    // The shared integer readers (D-NC2).
    // ---------------------------------------------------------------------------------------

    #[derive(Debug, Deserialize)]
    struct Required {
        #[serde(deserialize_with = "de_integral_u64")]
        v: u64,
    }

    #[derive(Debug, Deserialize)]
    struct Optional {
        #[serde(default, deserialize_with = "de_integral_opt_u64")]
        v: Option<u64>,
    }

    #[derive(Debug, Deserialize)]
    struct Floating {
        #[serde(default, deserialize_with = "de_opt_f64")]
        v: Option<f64>,
    }

    fn required_err(value: Value) -> String {
        serde_json::from_value::<Required>(json!({ "v": value }))
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn the_required_reader_accepts_both_encodings() {
        assert_eq!(
            serde_json::from_value::<Required>(json!({ "v": 300 }))
                .unwrap()
                .v,
            300
        );
        assert_eq!(
            serde_json::from_value::<Required>(json!({ "v": 300.0 }))
                .unwrap()
                .v,
            300
        );
    }

    /// Asserts the rejection message for `value` contains `expected`.
    fn assert_rejects(value: Value, expected: &str) {
        let err = required_err(value.clone());
        assert!(err.contains(expected), "for {value}, got: {err}");
    }

    #[test]
    fn the_required_reader_rejects_a_fractional_value_instead_of_truncating() {
        assert_rejects(
            json!(5.5),
            "expected an integer, got the fractional number 5.5",
        );
    }

    #[test]
    fn the_required_reader_rejects_a_negative_instead_of_saturating_to_zero() {
        assert_rejects(json!(-5), "expected a non-negative integer, got -5");
        assert_rejects(json!(-5.0), "expected a non-negative integer, got -5");
    }

    #[test]
    fn the_required_reader_rejects_an_out_of_range_value() {
        assert_rejects(
            json!(1e20),
            "expected an integer within the 64-bit range, got 100000000000000000000",
        );
    }

    #[test]
    fn the_required_reader_rejects_non_numbers() {
        assert_rejects(json!("300"), r#"expected an integer, got "300""#);
        assert_rejects(json!(true), "expected an integer, got true");
        assert_rejects(json!(null), "expected an integer, got null");
    }

    #[test]
    fn the_optional_reader_yields_none_only_for_absent_or_null() {
        assert_eq!(
            serde_json::from_value::<Optional>(json!({})).unwrap().v,
            None
        );
        assert_eq!(
            serde_json::from_value::<Optional>(json!({ "v": null }))
                .unwrap()
                .v,
            None
        );
        assert_eq!(
            serde_json::from_value::<Optional>(json!({ "v": 7.0 }))
                .unwrap()
                .v,
            Some(7)
        );
    }

    #[test]
    fn the_optional_reader_no_longer_swallows_a_bad_value() {
        let err = serde_json::from_value::<Optional>(json!({ "v": "seven" }))
            .unwrap_err()
            .to_string();
        assert!(err.contains("expected an integer"), "got: {err}");
        let err = serde_json::from_value::<Optional>(json!({ "v": 7.5 }))
            .unwrap_err()
            .to_string();
        assert!(err.contains("the fractional number 7.5"), "got: {err}");
    }

    #[test]
    fn the_float_reader_accepts_any_number_and_rejects_non_numbers() {
        assert_eq!(
            serde_json::from_value::<Floating>(json!({ "v": 1.5 }))
                .unwrap()
                .v,
            Some(1.5)
        );
        assert_eq!(
            serde_json::from_value::<Floating>(json!({ "v": 2 }))
                .unwrap()
                .v,
            Some(2.0)
        );
        assert_eq!(
            serde_json::from_value::<Floating>(json!({ "v": null }))
                .unwrap()
                .v,
            None
        );
        let err = serde_json::from_value::<Floating>(json!({ "v": "1.5" }))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(r#"expected a number, got "1.5""#),
            "got: {err}"
        );
    }

    #[test]
    fn the_probe_falls_back_rather_than_truncating() {
        assert_eq!(as_integral_u64(&json!(5)), Some(5));
        assert_eq!(as_integral_u64(&json!(5.0)), Some(5));
        assert_eq!(as_integral_u64(&json!(5.5)), None);
        assert_eq!(as_integral_u64(&json!(-5.0)), None);
        assert_eq!(as_integral_u64(&json!(1e20)), None);
        assert_eq!(as_integral_u64(&json!("5")), None);
    }

    #[cfg(feature = "credentials")]
    #[test]
    fn the_usize_reader_shares_the_rule() {
        #[derive(Debug, Deserialize)]
        struct Sized {
            #[serde(deserialize_with = "de_integral_usize")]
            v: usize,
        }
        assert_eq!(
            serde_json::from_value::<Sized>(json!({ "v": 2.0 }))
                .unwrap()
                .v,
            2
        );
        let err = serde_json::from_value::<Sized>(json!({ "v": 2.5 }))
            .unwrap_err()
            .to_string();
        assert!(err.contains("the fractional number 2.5"), "got: {err}");
    }
}
