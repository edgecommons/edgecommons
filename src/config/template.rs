//! # Configuration — template substitution
//!
//! **One-liner purpose**: Resolve `{ThingName}`, `{ComponentName}`,
//! `{ComponentFullName}`, and any `tags` key inside config strings.
//!
//! ## Overview
//! Used to expand placeholders in values such as log file paths and MQTT topics,
//! matching the substitution behavior of the Java/Python libraries.
//!
//! ## Semantics & Architecture
//! - Pure function over a [`Config`] snapshot; no I/O, no async, no panics.
//! - Error handling: infallible — unknown placeholders are left untouched.
//!
//! ## Usage Example
//! ```
//! use ggcommons::config::model::Config;
//! use ggcommons::config::template::resolve;
//! use serde_json::json;
//!
//! let cfg = Config::from_value("com.example.C", "t1", json!({})).unwrap();
//! assert_eq!(resolve(&cfg, "x/{ThingName}"), "x/t1");
//! ```
//!
//! ## Design Choices
//! Simple `String::replace` passes; substitution values are not yet sanitized for
//! file-path/topic injection — that hardening lands with the file/messaging
//! targets (closing the Java path-traversal/topic-injection concern).
//!
//! ## Safety & Panics
//! None.
//!
//! ## Related Modules
//! - [`super::model`].

use super::model::Config;

/// Replace known placeholders in `template` using values from `config`.
pub fn resolve(config: &Config, template: &str) -> String {
    let mut out = template
        .replace("{ThingName}", &config.thing_name)
        .replace("{ComponentName}", &config.component_name)
        .replace("{ComponentFullName}", &config.component_name);

    for (key, value) in &config.parsed.tags {
        if let Some(s) = value.as_str() {
            out = out.replace(&format!("{{{key}}}"), s);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn substitutes_builtins_and_tags() {
        let cfg = Config::from_value(
            "com.example.MyComponent",
            "thing-7",
            json!({ "tags": { "site": "factory-1" } }),
        )
        .unwrap();

        assert_eq!(
            resolve(&cfg, "heartbeat/{ThingName}/{ComponentName}"),
            "heartbeat/thing-7/com.example.MyComponent"
        );
        assert_eq!(resolve(&cfg, "/var/log/{site}.log"), "/var/log/factory-1.log");
    }

    #[test]
    fn leaves_unknown_placeholders_untouched() {
        let cfg = Config::from_value("c", "t", json!({})).unwrap();
        assert_eq!(resolve(&cfg, "{Unknown}"), "{Unknown}");
    }
}
