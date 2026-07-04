//! # Quality — the normalized sample-quality verdict
//!
//! **One-liner purpose**: The protocol-independent quality verdict every `data` sample carries
//! (DESIGN-class-facades §2.1, `docs/SOUTHBOUND.md` §3), mirroring the Java canonical
//! `com.mbreissi.ggcommons.facades.Quality`.

use serde::{Deserialize, Serialize};

/// The normalized, protocol-independent sample-quality verdict of the southbound contract. The
/// wire token is the enum's **UPPERCASE** name — `GOOD | BAD | UNCERTAIN` — carried verbatim on
/// every `data` sample.
///
/// [`crate::facades::DataFacade`] defaults an omitted sample quality to [`Quality::Good`]
/// (marking the synthesis with `qualityRaw:"unspecified"`), so a sample can never reach the bus
/// without a quality — the structural guarantee the facade exists to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Quality {
    /// The value is trustworthy (the default for a sample carrying a value with no verdict).
    Good,
    /// The value is not trustworthy (exception/timeout/failed read).
    Bad,
    /// The value is present but suspect (stale/partial).
    Uncertain,
}

impl Quality {
    /// The wire token — the UPPERCASE spelling exactly as it appears in a `data` sample.
    pub const fn wire(self) -> &'static str {
        match self {
            Quality::Good => "GOOD",
            Quality::Bad => "BAD",
            Quality::Uncertain => "UNCERTAIN",
        }
    }

    /// Resolves a wire token to its quality, or `None` when the token is outside the closed set.
    pub fn from_wire(token: &str) -> Option<Quality> {
        match token {
            "GOOD" => Some(Quality::Good),
            "BAD" => Some(Quality::Bad),
            "UNCERTAIN" => Some(Quality::Uncertain),
            _ => None,
        }
    }
}

impl std::fmt::Display for Quality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.wire())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_tokens_round_trip() {
        for q in [Quality::Good, Quality::Bad, Quality::Uncertain] {
            assert_eq!(Quality::from_wire(q.wire()), Some(q));
        }
        assert_eq!(Quality::from_wire("bogus"), None);
    }

    #[test]
    fn serde_uses_the_wire_tokens() {
        assert_eq!(serde_json::to_value(Quality::Good).unwrap(), serde_json::json!("GOOD"));
        assert_eq!(
            serde_json::from_value::<Quality>(serde_json::json!("UNCERTAIN")).unwrap(),
            Quality::Uncertain
        );
    }
}
