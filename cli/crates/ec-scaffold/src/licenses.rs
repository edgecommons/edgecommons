//! Embedded license texts for `--license` (DESIGN-cli-scaffold-parity SD-4).
//!
//! A scaffold is the *author's* component, so baking the EdgeCommons license choice into
//! third-party code is wrong — `--license` defaults to `none` and stamps no LICENSE. When an id
//! is passed, its canonical SPDX text is written into the generated project. The texts are
//! compiled in (from `licenses/*.txt`) so the writer stays offline, like everything else in
//! `component new`.

/// The canonical text for an SPDX id, or `None` if this build carries no text for it.
///
/// The three ids `--license` accepts (`BUSL-1.1`, `Apache-2.0`, `MIT`) are the ones with an
/// embedded text; any other id yields `None`.
#[must_use]
pub fn text(spdx: &str) -> Option<&'static str> {
    match spdx {
        "BUSL-1.1" => Some(include_str!("../licenses/BUSL-1.1.txt")),
        "Apache-2.0" => Some(include_str!("../licenses/Apache-2.0.txt")),
        "MIT" => Some(include_str!("../licenses/MIT.txt")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_supported_ids_have_embedded_texts() {
        for id in ["BUSL-1.1", "Apache-2.0", "MIT"] {
            let t = text(id).unwrap_or_else(|| panic!("no embedded text for {id}"));
            assert!(!t.trim().is_empty(), "{id} text must not be empty");
        }
        assert!(text("GPL-3.0").is_none());
        assert!(text("none").is_none());
    }

    #[test]
    fn the_texts_are_the_expected_licenses() {
        assert!(text("MIT").unwrap().contains("MIT License"));
        assert!(text("Apache-2.0").unwrap().contains("Apache License"));
        assert!(
            text("BUSL-1.1")
                .unwrap()
                .contains("Business Source License")
        );
    }
}
