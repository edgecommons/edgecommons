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
//! use edgecommons::config::model::Config;
//! use edgecommons::config::template::resolve;
//! use serde_json::json;
//!
//! let cfg = Config::from_value("com.example.C", "t1", json!({})).unwrap();
//! assert_eq!(resolve(&cfg, "x/{ThingName}"), "x/t1");
//! ```
//!
//! ## Design Choices
//! Simple `String::replace` passes. The **substituted values** (thing name,
//! component name, tag values) are sanitized before insertion so a hostile value
//! cannot inject path traversal (`..`, `/`, `\`) or MQTT topic wildcards (`+`, `#`)
//! into a resolved file path or topic — closing the Java path-traversal /
//! topic-injection concern (review M15). The template literal itself is left
//! intact, so legitimate separators in the template (e.g. `a/{ThingName}/b`) are
//! preserved.
//!
//! ## Safety & Panics
//! None.
//!
//! ## Related Modules
//! - [`super::model`].

use super::model::Config;

/// Replace known placeholders in `template` using values from `config`.
///
/// Each substituted value is passed through `sanitize` so it cannot break out of
/// the path or topic it is interpolated into.
pub fn resolve(config: &Config, template: &str) -> String {
    // `{ComponentName}` is the SHORT name (the segment after the last '.'),
    // `{ComponentFullName}` is the full name — matching Java's
    // ConfigManagerFactory (componentShortName = substring after last '.').
    let short_name = config
        .component_name
        .rsplit('.')
        .next()
        .unwrap_or(&config.component_name);
    let mut out = template
        .replace("{ThingName}", &sanitize(&config.thing_name))
        .replace("{ComponentFullName}", &sanitize(&config.component_name))
        .replace("{ComponentName}", &sanitize(short_name));

    for (key, value) in &config.parsed.tags {
        if let Some(s) = value.as_str() {
            out = out.replace(&format!("{{{key}}}"), &sanitize(s));
        }
    }
    out
}

/// Neutralize characters in a substituted value that are dangerous in a file path
/// or MQTT topic: path separators (`/`, `\`), traversal dots, MQTT wildcards
/// (`+`, `#`), and control characters (Unicode `Cc` — C0, DEL, and C1) are each
/// replaced with `_`.
///
/// Applied only to interpolated values, never to the surrounding template, so
/// structural separators in the template are preserved.
///
/// Public because it is also the **normative UNS token sanitizer**
/// (UNS-CANONICAL-DESIGN §2.2 rule 1 / D-U26): the [`crate::uns`] token rule is
/// exactly this blacklist, so "sanitized ⇒ publishable" holds. The identity
/// resolution ([`crate::config::model::Config::identity`]) and the metric
/// `messaging` target (metric name → `metric/{name}` channel token) use it.
pub fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' | '\\' | '+' | '#' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect::<String>()
        // Collapse traversal sequences (e.g. "..") that remain after separator replacement.
        .replace("..", "_")
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
            "heartbeat/thing-7/MyComponent"
        );
        assert_eq!(resolve(&cfg, "/var/log/{site}.log"), "/var/log/factory-1.log");
    }

    #[test]
    fn component_name_is_short_and_full_name_is_full() {
        let cfg = Config::from_value("com.example.MyComponent", "t", json!({})).unwrap();
        // {ComponentName} is the segment after the last '.', {ComponentFullName} is the whole name.
        assert_eq!(resolve(&cfg, "{ComponentName}"), "MyComponent");
        assert_eq!(resolve(&cfg, "{ComponentFullName}"), "com.example.MyComponent");
        // A name with no dots: short == full.
        let cfg2 = Config::from_value("Simple", "t", json!({})).unwrap();
        assert_eq!(resolve(&cfg2, "{ComponentName}"), "Simple");
        assert_eq!(resolve(&cfg2, "{ComponentFullName}"), "Simple");
    }

    #[test]
    fn leaves_unknown_placeholders_untouched() {
        let cfg = Config::from_value("c", "t", json!({})).unwrap();
        assert_eq!(resolve(&cfg, "{Unknown}"), "{Unknown}");
    }

    #[test]
    fn sanitizes_path_traversal_and_topic_wildcards_in_values() {
        // A hostile thing name / tag value must not break out of the path or topic.
        let cfg = Config::from_value(
            "com.example.C",
            "../../etc/passwd",
            json!({ "tags": { "evil": "a/+/#" } }),
        )
        .unwrap();

        // Path separators and traversal in the value are neutralized; the template's
        // own separators are preserved.
        assert_eq!(resolve(&cfg, "/logs/{ThingName}.log"), "/logs/____etc_passwd.log");
        assert_eq!(resolve(&cfg, "t/{evil}/x"), "t/a____/x");
    }

    #[test]
    fn preserves_template_separators_and_clean_values() {
        let cfg = Config::from_value("com.example.MyComponent", "thing-7", json!({})).unwrap();
        // Dotted component names are fine (no traversal sequence) and template
        // slashes are kept.
        assert_eq!(
            resolve(&cfg, "{ThingName}/{ComponentName}/metric"),
            "thing-7/MyComponent/metric"
        );
    }
}
