//! # Channel — publish-channel routing address
//!
//! **One-liner purpose**: The uniform `{ local, northbound, stream:<name> }` routing target the
//! publish facades resolve on (DESIGN-class-facades §4, `DESIGN-channels.md`), mirroring the Java
//! canonical `com.mbreissi.edgecommons.facades.Channel`.

use crate::error::{EdgeCommonsError, Result};

/// A publish-channel address: the uniform `{ local, northbound, stream:<name> }` routing target.
///
/// - [`Channel::Local`] — the local bus (`messaging().publish`). The default.
/// - [`Channel::Northbound`] — the northbound/cloud broker (`messaging().publish_northbound`).
/// - [`Channel::Stream`] — the named durable telemetry stream
///   (`streams().stream(name).append(...)`); **only [`crate::facades::DataFacade`] honors it** —
///   `events()`/`app()` reject a stream channel (they are low-rate control-plane, not bulk
///   telemetry).
///
/// [`Channel::from_config`] parses the config `publish.channel` string (DESIGN-class-facades §4
/// Option C): `"local"`, `"northbound"`, or `"stream:<name>"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Channel {
    /// The local bus channel (the default).
    Local,
    /// The northbound/cloud channel.
    Northbound,
    /// The named durable-stream channel (`data()` only).
    Stream(String),
}

impl Channel {
    /// The named-durable-stream channel.
    ///
    /// # Errors
    /// [`EdgeCommonsError::Facade`] when `name` is empty.
    pub fn stream(name: impl Into<String>) -> Result<Channel> {
        let name = name.into();
        if name.is_empty() {
            return Err(EdgeCommonsError::Facade(
                "stream channel name must be non-empty".to_string(),
            ));
        }
        Ok(Channel::Stream(name))
    }

    /// Whether this is the [`Channel::Stream`] variant (the check `events()`/`app()` use to
    /// reject a stream routing override).
    pub const fn is_stream(&self) -> bool {
        matches!(self, Channel::Stream(_))
    }

    /// Parses a config `publish.channel` string into a channel (DESIGN-class-facades §4, Option
    /// C). Recognized: `"local"` → [`Channel::Local`]; `"northbound"` →
    /// [`Channel::Northbound`]; `"stream:<name>"` → [`Channel::Stream`]. Any other
    /// (or empty) value yields `None` so the caller can fall through to its own default.
    pub fn from_config(value: &str) -> Option<Channel> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower == "local" {
            return Some(Channel::Local);
        }
        if lower == "northbound" {
            return Some(Channel::Northbound);
        }
        if let Some(name) = trimmed.strip_prefix("stream:") {
            return if name.is_empty() {
                None
            } else {
                Some(Channel::Stream(name.to_string()))
            };
        }
        None
    }
}

impl std::fmt::Display for Channel {
    /// `"local"` / `"northbound"` / `"stream:<name>"` — the config-string form.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Channel::Local => f.write_str("local"),
            Channel::Northbound => f.write_str("northbound"),
            Channel::Stream(name) => write!(f, "stream:{name}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_config_recognizes_all_forms() {
        assert_eq!(Channel::from_config("local"), Some(Channel::Local));
        assert_eq!(
            Channel::from_config("northbound"),
            Some(Channel::Northbound)
        );
        assert_eq!(
            Channel::from_config("stream:hot"),
            Some(Channel::Stream("hot".to_string()))
        );
        assert_eq!(Channel::from_config(""), None);
        assert_eq!(Channel::from_config("iotcore"), None);
        assert_eq!(Channel::from_config("iot_core"), None);
        assert_eq!(Channel::from_config("stream:"), None);
        assert_eq!(Channel::from_config("bogus"), None);
    }

    #[test]
    fn stream_rejects_empty_name() {
        assert!(Channel::stream("").is_err());
        assert!(Channel::stream("hot").is_ok());
    }

    #[test]
    fn is_stream_and_display() {
        assert!(Channel::stream("hot").unwrap().is_stream());
        assert!(!Channel::Local.is_stream());
        assert_eq!(Channel::Northbound.to_string(), "northbound");
        assert_eq!(Channel::Stream("hot".into()).to_string(), "stream:hot");
    }
}
