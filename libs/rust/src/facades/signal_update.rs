//! # SignalUpdate — the constructed `SouthboundSignalUpdate` body (value type + builder)
//!
//! **One-liner purpose**: The value object [`DataFacade::publish`](super::DataFacade::publish)
//! consumes — the thing that replaces an adapter's hand-assembled JSON body — plus its fluent
//! [`SignalUpdateBuilder`], mirroring the Java canonical
//! `com.mbreissi.edgecommons.facades.SignalUpdate`.

use serde_json::Value;

use super::{Channel, Quality};

/// One sample: a measured `value` plus the optional quality/timestamp parts.
///
/// A `None` `value` is a fail-fast [`crate::EdgeCommonsError::Facade`] at
/// [`DataFacade::publish`](super::DataFacade::publish) (a quality-only sample is not a sample —
/// pass `Quality::Bad`/`Quality::Uncertain` for a failed read instead). A `None` `quality` is
/// defaulted to [`Quality::Good`] by the facade; a `None` `server_ts` is filled with now;
/// `source_ts` is never synthesized; `quality_raw` is a synthetic `"unspecified"` marker when (and
/// only when) the quality was defaulted, else passed through verbatim.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sample {
    /// The measured value (JSON-native: number/boolean/string/array/object). Required at publish
    /// time — `None` here models "no value supplied", the one hard reject inside a sample.
    pub value: Option<Value>,
    /// The normalized quality, or `None` to default to [`Quality::Good`].
    pub quality: Option<Quality>,
    /// The native status code, or `None`.
    pub quality_raw: Option<String>,
    /// The device/field ISO-8601 timestamp, or `None` (never synthesized).
    pub source_ts: Option<String>,
    /// The protocol-server ISO-8601 timestamp, or `None` to default to now.
    pub server_ts: Option<String>,
}

impl Sample {
    /// A value-only sample: quality defaults to [`Quality::Good`], `server_ts` to now.
    pub fn new(value: impl Into<Value>) -> Sample {
        Sample { value: Some(value.into()), ..Default::default() }
    }

    /// A value + explicit quality sample (`server_ts` defaults to now).
    pub fn with_quality(value: impl Into<Value>, quality: Quality) -> Sample {
        Sample { value: Some(value.into()), quality: Some(quality), ..Default::default() }
    }

    /// A value + quality + device-timestamp sample (`server_ts` defaults to now).
    pub fn with_source_ts(
        value: impl Into<Value>,
        quality: Quality,
        source_ts: impl Into<String>,
    ) -> Sample {
        Sample {
            value: Some(value.into()),
            quality: Some(quality),
            source_ts: Some(source_ts.into()),
            ..Default::default()
        }
    }

    /// Sets the native status code (fluent).
    pub fn quality_raw(mut self, raw: impl Into<String>) -> Sample {
        self.quality_raw = Some(raw.into());
        self
    }

    /// Sets an explicit `server_ts` instead of the "now" default (fluent).
    pub fn server_ts(mut self, ts: impl Into<String>) -> Sample {
        self.server_ts = Some(ts.into());
        self
    }
}

/// The constructed `SouthboundSignalUpdate` body (DESIGN-class-facades §2.1,
/// `docs/SOUTHBOUND.md` §2) — the value object that replaces an adapter's hand-assembled JSON
/// body. Holds the raw inputs: an optional `device` block; the `signal` with its stable id plus
/// optional name/address; the samples; the sanitized-into-a-channel `signal_path`; an optional
/// per-call [`Channel`] override.
///
/// Obtain a builder from [`SignalUpdate::builder`] or [`DataFacade::signal`](super::DataFacade::signal)
/// and terminate with [`SignalUpdateBuilder::build`], then
/// [`DataFacade::publish`](super::DataFacade::publish). `signal_id` is the only structural
/// requirement — a missing/empty one is a fail-fast [`crate::EdgeCommonsError::Facade`] at publish
/// (DESIGN-class-facades §5.2), never a dropped message.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SignalUpdate {
    /// The optional `device` block (`{adapter, instance, endpoint}`), or `None`.
    pub device: Option<Value>,
    /// The stable `signal.id` (REQUIRED at publish; the consumer key).
    pub signal_id: Option<String>,
    /// The human `signal.name`, or `None`.
    pub signal_name: Option<String>,
    /// The protocol-native `signal.address`, or `None`.
    pub signal_address: Option<Value>,
    /// The samples (the facade rejects an empty list at publish).
    pub samples: Vec<Sample>,
    /// The channel path (the `data/{signal_path}` tail); `None` means "use signal_id".
    pub signal_path: Option<String>,
    /// The per-call [`Channel`] override, or `None` (resolve config default ▸ LOCAL).
    pub via: Option<Channel>,
}

impl SignalUpdate {
    /// Starts a detached builder (no bound facade — this is a plain value builder in Rust; call
    /// [`DataFacade::publish`](super::DataFacade::publish) with the built value).
    pub fn builder() -> SignalUpdateBuilder {
        SignalUpdateBuilder::default()
    }

    /// The effective channel path: [`Self::signal_path`] when set, else [`Self::signal_id`].
    pub fn effective_signal_path(&self) -> Option<&str> {
        self.signal_path.as_deref().or(self.signal_id.as_deref())
    }
}

/// The fluent `SouthboundSignalUpdate` builder — `signal_id(id).name(n).address(a).device(d)
/// .sample(...).signal_path(p).build()`. Reused across all four languages (DESIGN-class-facades
/// §2.1/§6).
#[derive(Debug, Clone, Default)]
pub struct SignalUpdateBuilder {
    update: SignalUpdate,
}

impl SignalUpdateBuilder {
    /// Sets the stable `signal.id` (REQUIRED at publish — the consumer key).
    pub fn signal_id(mut self, id: impl Into<String>) -> Self {
        self.update.signal_id = Some(id.into());
        self
    }

    /// Sets the human `signal.name`.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.update.signal_name = Some(name.into());
        self
    }

    /// Sets the protocol-native `signal.address`.
    pub fn address(mut self, address: Value) -> Self {
        self.update.signal_address = Some(address);
        self
    }

    /// Sets a pre-built `device` block.
    pub fn device(mut self, device: Value) -> Self {
        self.update.device = Some(device);
        self
    }

    /// Sets the `device` block from its three parts (mirrors the Java/Python `device(adapter,
    /// instance, endpoint)` overload).
    pub fn device_parts(self, adapter: &str, instance: &str, endpoint: &str) -> Self {
        self.device(serde_json::json!({
            "adapter": adapter,
            "instance": instance,
            "endpoint": endpoint,
        }))
    }

    /// Appends one sample.
    pub fn sample(mut self, sample: Sample) -> Self {
        self.update.samples.push(sample);
        self
    }

    /// Appends a batch of samples (the coalesced-publish path).
    pub fn samples(mut self, samples: impl IntoIterator<Item = Sample>) -> Self {
        self.update.samples.extend(samples);
        self
    }

    /// Sets the channel path — the `data/{signal_path}` tail (each `/`-separated token is
    /// sanitized into a UNS token by the facade). When unset, the stable `signal_id` is used as
    /// the path (D-U15's sanitized-path-vs-stable-id split still holds — the body's raw id rides
    /// untouched).
    pub fn signal_path(mut self, signal_path: impl Into<String>) -> Self {
        self.update.signal_path = Some(signal_path.into());
        self
    }

    /// Sets a per-call [`Channel`] override (LOCAL / NORTHBOUND / stream).
    pub fn via(mut self, channel: Channel) -> Self {
        self.update.via = Some(channel);
        self
    }

    /// Builds the immutable [`SignalUpdate`].
    pub fn build(self) -> SignalUpdate {
        self.update
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_assembles_the_update() {
        let update = SignalUpdate::builder()
            .signal_id("temp")
            .name("Temperature")
            .signal_path("press12/temperature")
            .sample(Sample::new(21.5))
            .build();
        assert_eq!(update.signal_id.as_deref(), Some("temp"));
        assert_eq!(update.signal_name.as_deref(), Some("Temperature"));
        assert_eq!(update.effective_signal_path(), Some("press12/temperature"));
        assert_eq!(update.samples.len(), 1);
    }

    #[test]
    fn effective_signal_path_falls_back_to_signal_id() {
        let update = SignalUpdate::builder().signal_id("temp").sample(Sample::new(1)).build();
        assert_eq!(update.effective_signal_path(), Some("temp"));
    }

    #[test]
    fn sample_convenience_constructors() {
        let s = Sample::with_source_ts(21.5, Quality::Good, "2026-07-01T11:59:59Z")
            .quality_raw("Good")
            .server_ts("2026-07-01T11:59:59.5Z");
        assert_eq!(s.value, Some(Value::from(21.5)));
        assert_eq!(s.quality, Some(Quality::Good));
        assert_eq!(s.source_ts.as_deref(), Some("2026-07-01T11:59:59Z"));
        assert_eq!(s.quality_raw.as_deref(), Some("Good"));
        assert_eq!(s.server_ts.as_deref(), Some("2026-07-01T11:59:59.5Z"));
    }
}
