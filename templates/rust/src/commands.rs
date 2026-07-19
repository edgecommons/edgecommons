//! # Custom command logic — the unit-tested seam of this scaffold
//!
//! A service scaffold is mostly runtime wiring (build the runtime, tick a demo loop, publish), which
//! is exercised by the scaffold→build gate and HOST smoke rather than unit tests. The one piece of
//! *pure* behavior is the custom `set-greeting` verb's argument handling: validate the body and swap
//! the stored greeting. It lives here, on its own, so it is unit-tested and stays in the coverage
//! denominator — while the async registration/loop glue in `app.rs`/`main.rs` (which needs a live
//! `EdgeCommons` runtime) is the excluded seam. Grow this module as your component grows real logic.

use std::sync::Mutex;

use edgecommons::prelude::CommandError;
use serde_json::{json, Value};

/// Apply the `set-greeting` verb to the shared greeting state.
///
/// Returns the `{previousGreeting, greeting}` change on success, or a `BAD_ARGS`
/// [`CommandError`] when the request body does not carry a string `greeting`. The registration glue
/// in [`crate::app::configure_commands`] is the only caller; keeping the decision here (not in the
/// async handler closure) is what makes it testable without a running command inbox.
///
/// # Errors
/// Returns `BAD_ARGS` when `body.greeting` is missing or not a string.
pub fn apply_set_greeting(current: &Mutex<String>, body: &Value) -> Result<Option<Value>, CommandError> {
    let next = match body.get("greeting").and_then(Value::as_str) {
        Some(value) => value.to_string(),
        None => {
            return Err(CommandError::new(
                "BAD_ARGS",
                "expected a JSON body {\"greeting\": \"<text>\"}",
            ));
        }
    };
    let previous = {
        let mut guard = current.lock().expect("greeting mutex poisoned");
        std::mem::replace(&mut *guard, next.clone())
    };
    Ok(Some(json!({ "previousGreeting": previous, "greeting": next })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_valid_greeting_swaps_the_state_and_reports_the_change() {
        let state = Mutex::new("old".to_string());
        let out = apply_set_greeting(&state, &json!({ "greeting": "new" })).unwrap().unwrap();
        assert_eq!(out["previousGreeting"], "old");
        assert_eq!(out["greeting"], "new");
        assert_eq!(&*state.lock().unwrap(), "new", "the state now reflects the command");
    }

    #[test]
    fn a_missing_greeting_is_rejected_with_bad_args_and_leaves_the_state_untouched() {
        let state = Mutex::new("keep".to_string());
        let err = apply_set_greeting(&state, &json!({})).unwrap_err();
        assert_eq!(err.code, "BAD_ARGS");
        assert_eq!(&*state.lock().unwrap(), "keep", "a rejected command must not mutate state");
    }

    #[test]
    fn a_non_string_greeting_is_also_bad_args() {
        let state = Mutex::new("keep".to_string());
        let err = apply_set_greeting(&state, &json!({ "greeting": 42 })).unwrap_err();
        assert_eq!(err.code, "BAD_ARGS");
    }
}
