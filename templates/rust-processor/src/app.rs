//! # <<COMPONENTNAME>> — a processing component
//!
//! A **processor** subscribes to messages, transforms them, and forwards the result. This
//! scaffold wires that shape end to end; the transformation itself lives in [`crate::proc`],
//! which is where your code goes.
//!
//! ```text
//!   subscribe(filter) ──► bounded queue ──► one task per route ──► publish
//!                                              (Pipeline)          local | northbound
//! ```
//!
//! Each entry of `component.instances[]` is **one route**: topic filters, a pipeline of stages,
//! and a target. Routes are independent — one task each — so a slow route cannot stall another,
//! and the per-key state inside a stage needs no lock.
//!
//! ## Why a processor uses `messaging()` and not `data()`
//!
//! Worth reading twice, because it is the mistake this archetype invites. The `data()` facade is
//! for a component that *produces* readings: it mints its own topic from a signal id and imposes
//! the `SouthboundSignalUpdate` body. A processor is **payload-agnostic** — it republishes what
//! it was handed, on a topic its route names. Routing that through `data()` would rewrite both
//! the topic and the body, which is exactly what a republisher must not do. So: raw
//! `gg.messaging()`, and topics from config.
//!
//! ## Two guards that are not optional
//!
//! * **Self-echo.** A processor that publishes onto a class it also subscribes to will consume
//!   its own output, reprocess it, republish it, and saturate the device. [`is_self_echo`] drops
//!   anything carrying our own identity.
//! * **Identity restamp.** What we publish is *ours*. Without the restamp the fleet cannot tell
//!   who emitted a message — and the self-echo guard downstream cannot work either.

use std::sync::atomic::AtomicU64;

use edgecommons::messaging::Message;
use serde::Deserialize;

use crate::proc::{CountPerTick, FieldEquals, Processor};

/// Where a route's output goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    /// The device-local bus — the common case; another component on this device consumes it.
    #[default]
    Local,
    /// Straight out to the northbound broker.
    Northbound,
}

/// One route == one entry of `component.instances[]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteConfig {
    pub id: String,
    /// Topic filters to subscribe to. Wildcards are fine: `ecv1/+/+/+/data/#`.
    #[serde(default)]
    pub subscribe: Vec<String>,
    /// The topic the result is published on.
    pub publish_topic: String,
    #[serde(default)]
    pub target: Target,
    /// The stages, in order. An empty pipeline is a pass-through republisher.
    #[serde(default)]
    pub pipeline: Vec<StageConfig>,
    /// How many messages may be queued for this route before new ones are dropped.
    ///
    /// Bounded on purpose. An unbounded queue does not remove backpressure — it relocates the
    /// failure to the heap, and by the time you notice you have lost the ability to report it.
    #[serde(default = "default_max_queue")]
    pub max_queue: usize,
    /// How often stateful stages are ticked, in milliseconds.
    #[serde(default = "default_tick_ms")]
    pub tick_ms: u64,
}

fn default_max_queue() -> usize {
    256
}
fn default_tick_ms() -> u64 {
    10_000
}

/// A stage, as named in config. Add a variant here as you add a stage.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StageConfig {
    /// Keep only messages whose dotted body path equals a value.
    FieldEquals { path: String, value: serde_json::Value },
    /// Accumulate arrivals; emit a rollup every tick.
    ///
    /// A struct variant with no fields, not a unit variant: `{"countPerTick": {}}` is what the
    /// config and the schema say, and a unit variant would demand the bare string instead.
    CountPerTick {},
}

impl StageConfig {
    pub(crate) fn build(&self) -> Box<dyn Processor> {
        match self {
            Self::FieldEquals { path, value } => {
                Box::new(FieldEquals { path: path.clone(), value: value.clone() })
            }
            Self::CountPerTick {} => Box::new(CountPerTick { seen: 0, last: None }),
        }
    }
}

/// Counters, reported as a metric each interval.
#[derive(Default)]
pub struct Stats {
    pub received: AtomicU64,
    pub published: AtomicU64,
    /// Dropped because a route's queue was full. **Never let this be invisible** — a processor
    /// that silently discards messages is worse than one that crashes.
    pub dropped: AtomicU64,
    pub errors: AtomicU64,
}

/// Would consuming this message mean consuming our own output?
#[must_use]
pub fn is_self_echo(msg: &Message, my_path: &str, my_component: &str) -> bool {
    msg.identity
        .as_ref()
        .is_some_and(|id| id.path() == my_path && id.component() == my_component)
}

/// Apply `component.global.defaults` to a route that did not set the key itself.
///
/// The schema promises this knob; a knob the code ignores is worse than no knob at all.
pub(crate) fn apply_defaults(route: &mut RouteConfig, defaults: &serde_json::Value) {
    if route.tick_ms == default_tick_ms() {
        if let Some(v) = defaults.get("tickMs").and_then(serde_json::Value::as_u64) {
            route.tick_ms = v;
        }
    }
    if route.max_queue == default_max_queue() {
        if let Some(v) = defaults.get("maxQueue").and_then(serde_json::Value::as_u64) {
            route.max_queue = usize::try_from(v).unwrap_or_else(|_| default_max_queue());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use crate::proc::{Out, Pipeline};

    #[test]
    fn a_route_parses_from_its_instance_config() {
        let route: RouteConfig = serde_json::from_value(json!({
            "id": "temps",
            "subscribe": ["ecv1/+/+/+/data/#"],
            "publishTopic": "ecv1/gw01/<<BINNAME>>/main/data/rollup",
            "target": "northbound",
            "pipeline": [
                { "fieldEquals": { "path": "signal.id", "value": "temp-1" } },
                { "countPerTick": {} }
            ],
            "tickMs": 5000
        }))
        .unwrap();

        assert_eq!(route.id, "temps");
        assert_eq!(route.target, Target::Northbound);
        assert_eq!(route.pipeline.len(), 2);
        assert_eq!(route.tick_ms, 5_000);
        assert_eq!(route.max_queue, 256, "the queue is bounded by default");
    }

    #[test]
    fn the_defaults_are_the_common_case() {
        let route: RouteConfig =
            serde_json::from_value(json!({ "id": "r", "publishTopic": "t" })).unwrap();
        assert_eq!(route.target, Target::Local, "the device-local bus is the common target");
        assert!(route.pipeline.is_empty(), "no stages == a pass-through republisher");
    }

    #[test]
    fn an_unknown_config_key_is_rejected_rather_than_ignored() {
        // deny_unknown_fields: a typo'd route key is a mistake, not a no-op.
        let bad = serde_json::from_value::<RouteConfig>(json!({
            "id": "r", "publishTopic": "t", "pipelnie": []
        }));
        assert!(bad.is_err());
    }

    #[test]
    fn each_stage_variant_builds_its_processor() {
        // Building both variants covers the construction seam the run-loop composes; the built
        // processors' behavior is tested against the pipeline in `src/proc.rs`.
        let filter = StageConfig::FieldEquals { path: "signal.id".into(), value: json!("temp-1") };
        let rollup = StageConfig::CountPerTick {};
        // A pass-through pipeline built from both must accept a message without panicking.
        let mut pipeline = Pipeline::new(vec![filter.build(), rollup.build()]);
        let out = pipeline.run(Out::new(), Some(1));
        // No arrivals + a tick: nothing to emit yet, but the stages were constructed and ticked.
        assert!(out.is_empty());
    }

    #[test]
    fn defaults_fill_only_the_keys_a_route_left_at_their_default() {
        // A route that set neither key inherits both from component.global.defaults.
        let mut inherit: RouteConfig =
            serde_json::from_value(json!({ "id": "r", "publishTopic": "t" })).unwrap();
        apply_defaults(&mut inherit, &json!({ "tickMs": 2500, "maxQueue": 64 }));
        assert_eq!(inherit.tick_ms, 2_500);
        assert_eq!(inherit.max_queue, 64);

        // A route that set its own values keeps them — a default never overrides an explicit choice.
        let mut explicit: RouteConfig = serde_json::from_value(
            json!({ "id": "r", "publishTopic": "t", "tickMs": 7000, "maxQueue": 512 }),
        )
        .unwrap();
        apply_defaults(&mut explicit, &json!({ "tickMs": 2500, "maxQueue": 64 }));
        assert_eq!(explicit.tick_ms, 7_000, "an explicit tickMs is not overridden by a default");
        assert_eq!(explicit.max_queue, 512, "an explicit maxQueue is not overridden by a default");
    }

    #[test]
    fn self_echo_is_detected_by_our_own_identity_and_nothing_else() {
        use edgecommons::messaging::MessageBuilder;
        use edgecommons::prelude::Config;

        let config = Config::from_value(
            "com.example.MyProcessor",
            "thing-1",
            json!({ "metricEmission": { "target": "log", "namespace": "test" } }),
        )
        .unwrap();
        let (me_path, me_component) =
            (config.identity().path().to_string(), config.identity().component().to_string());

        // Stamped with OUR identity → an echo we must drop.
        let ours = MessageBuilder::new("T", "1.0").from_config(&config).payload(json!({})).build();
        assert!(is_self_echo(&ours, &me_path, &me_component));

        // Stamped, but by someone else → not an echo.
        assert!(!is_self_echo(&ours, "ecv1/other/thing", &me_component));

        // No identity at all → not an echo (a producer that never stamped identity).
        let anon = MessageBuilder::new("T", "1.0").payload(json!({})).build();
        assert!(!is_self_echo(&anon, &me_path, &me_component));
    }
}
