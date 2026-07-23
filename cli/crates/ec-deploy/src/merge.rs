//! The shipped hierarchical-config merge contract: objects merge recursively by key, arrays
//! and scalars replace wholesale, later layers win, and key positions are stable — an
//! overridden key keeps its first-seen position, a new key appends (`preserve_order`).
//!
//! DESIGN-cli §8.3(2) demands the renderer's effective config not be a semantically second
//! merge. Rather than linking the runtime crate into the kernel, conformance is proven
//! against the **shared cross-language test vectors** (`hierarchical-config-test-vectors/
//! merge.json`) — the same vectors the four runtime libraries must pass — in this module's
//! tests.

use serde_json::{Map, Value};

pub fn deep_merge(base: &mut Map<String, Value>, overlay: &Map<String, Value>) {
    for (key, incoming) in overlay {
        match (base.get_mut(key), incoming) {
            (Some(Value::Object(existing)), Value::Object(over)) => deep_merge(existing, over),
            _ => {
                base.insert(key.clone(), incoming.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Conformance with the shared merge vectors (DESIGN-cli §8.3(2)).
    #[test]
    fn merge_matches_the_shared_conformance_vectors() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../hierarchical-config-test-vectors/merge.json");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("shared vectors missing at {}: {e}", path.display()));
        let vectors: Value = serde_json::from_str(&text).unwrap();
        let cases = vectors["cases"].as_array().expect("vectors carry {cases:[..]}");
        assert!(!cases.is_empty(), "no merge vectors found");
        for case in cases {
            let name = case["name"].as_str().unwrap_or("<unnamed>");
            let layers = case["input"]["layers"]
                .as_array()
                .unwrap_or_else(|| panic!("case {name}: missing input.layers"));
            let expected = &case["expected"]["effective"];
            let mut acc = Map::new();
            for layer in layers {
                let obj = layer["config"]
                    .as_object()
                    .unwrap_or_else(|| panic!("case {name}: layer without object config"));
                deep_merge(&mut acc, obj);
            }
            assert_eq!(
                &Value::Object(acc),
                expected,
                "merge vector '{name}' diverges from the shared contract"
            );
        }
    }
}
