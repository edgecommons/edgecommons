//! # Severity — the operator-event severity taxonomy
//!
//! **One-liner purpose**: The `evt` severity taxonomy (DESIGN-class-facades §2.2), mirroring the
//! Java canonical `com.mbreissi.edgecommons.facades.Severity`.

use serde::{Deserialize, Serialize};

/// The operator-event severity taxonomy. The wire token is the enum's **lowercase** name —
/// `critical | warning | info | debug` — and it is the **first channel token** of every `evt`
/// publish: [`crate::facades::EventsFacade`] derives the channel `evt/{severity}/{type}` from the
/// body's own severity + type, so the topic and the body can never disagree. A console subscribes
/// `ecv1/+/+/+/evt/critical/#` for just alarms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// An alarm-grade condition demanding operator attention (the `raise_alarm` default).
    Critical,
    /// A degraded but non-critical condition.
    Warning,
    /// An informational event (the message-only `emit` default).
    Info,
    /// A diagnostic event.
    Debug,
}

impl Severity {
    /// The wire token — the lowercase spelling, the `evt` channel's first token.
    pub const fn wire(self) -> &'static str {
        match self {
            Severity::Critical => "critical",
            Severity::Warning => "warning",
            Severity::Info => "info",
            Severity::Debug => "debug",
        }
    }

    /// Resolves a lowercase wire token to its severity, or `None` when outside the closed set.
    pub fn from_wire(token: &str) -> Option<Severity> {
        match token {
            "critical" => Some(Severity::Critical),
            "warning" => Some(Severity::Warning),
            "info" => Some(Severity::Info),
            "debug" => Some(Severity::Debug),
            _ => None,
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.wire())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_tokens_round_trip() {
        for s in [Severity::Critical, Severity::Warning, Severity::Info, Severity::Debug] {
            assert_eq!(Severity::from_wire(s.wire()), Some(s));
        }
        assert_eq!(Severity::from_wire("bogus"), None);
    }

    #[test]
    fn serde_uses_the_wire_tokens() {
        assert_eq!(serde_json::to_value(Severity::Critical).unwrap(), serde_json::json!("critical"));
        assert_eq!(
            serde_json::from_value::<Severity>(serde_json::json!("debug")).unwrap(),
            Severity::Debug
        );
    }
}
