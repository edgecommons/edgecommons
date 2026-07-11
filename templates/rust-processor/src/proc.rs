//! # The pipeline: what a *processor* is
//!
//! A processor **subscribes**, **transforms**, and **forwards**. That is the whole archetype,
//! and it lives in three types:
//!
//! * [`ProcMsg`] — the unit that flows through the pipeline: a message plus the topic it
//!   arrived on.
//! * [`Processor`] — one stage. It takes a message and returns **zero or more** messages, so a
//!   stage can filter (return nothing), map (return one), or fan out (return several).
//! * [`Pipeline`] — an ordered chain of stages. The output of each stage is the input of the
//!   next.
//!
//! ## Why stages return `Out` and not `Option<ProcMsg>`
//!
//! A filter drops, a projection maps, an aggregator emits on a timer rather than on arrival.
//! `0..N` covers all three without a special case, and it is what lets [`Processor::on_tick`]
//! exist: a *stateful* stage (a window, a debounce, a batch) accumulates on `process` and emits
//! on `on_tick`, so time-driven output is not a different mechanism from data-driven output.
//!
//! ## One task per route, so state needs no lock
//!
//! Each route owns its `Pipeline` in a single task. That is deliberate: per-key state inside a
//! stage is plain `&mut self` with no `Mutex` anywhere, which is what makes a stateful stage
//! cheap to write correctly.

use edgecommons::messaging::Message;
use smallvec::SmallVec;

/// A message in flight, and the topic it arrived on.
///
/// The topic is carried because a stage may want to route on it, and because the dispatcher
/// needs it to decide where the result goes.
#[derive(Debug, Clone)]
pub struct ProcMsg {
    /// The topic it arrived on. The demo stages ignore it; yours may want to route on it.
    #[allow(dead_code)]
    pub topic: String,
    pub msg: Message,
}

impl ProcMsg {
    #[must_use]
    pub fn new(topic: impl Into<String>, msg: Message) -> Self {
        Self { topic: topic.into(), msg }
    }
}

/// What a stage emits: zero, one, or many messages.
///
/// Inline capacity of 1 because the overwhelming majority of stages are 1-in/1-out or 1-in/0-out,
/// and a heap allocation per message on the hot path is a tax worth not paying.
pub type Out = SmallVec<[ProcMsg; 1]>;

/// One stage of the pipeline. **This is the trait you implement.**
pub trait Processor: Send {
    /// Handle one inbound message. Return what should continue downstream.
    fn process(&mut self, m: ProcMsg) -> Out;

    /// Called periodically, for stages that emit on time rather than on arrival (a window, a
    /// batch, a debounce). The default is to emit nothing — a stateless stage ignores time.
    fn on_tick(&mut self, _now_ms: u64) -> Out {
        Out::new()
    }
}

/// An ordered chain of stages.
pub struct Pipeline {
    stages: Vec<Box<dyn Processor>>,
}

impl Pipeline {
    #[must_use]
    pub fn new(stages: Vec<Box<dyn Processor>>) -> Self {
        Self { stages }
    }

    /// Run a batch through every stage in order.
    ///
    /// When `tick_ms` is `Some`, each stage additionally gets an [`Processor::on_tick`] after its
    /// data pass, and whatever it emits joins the batch flowing downstream — so a window closing
    /// in stage 1 is still projected by stage 2 on the same pass, rather than waiting for the
    /// next message to shake it loose.
    pub fn run(&mut self, input: Out, tick_ms: Option<u64>) -> Out {
        let mut carried = input;
        for stage in &mut self.stages {
            let mut next = Out::new();
            for m in carried.drain(..) {
                next.extend(stage.process(m));
            }
            if let Some(now) = tick_ms {
                next.extend(stage.on_tick(now));
            }
            carried = next;
        }
        carried
    }
}

// --- Demo stages -----------------------------------------------------------------------------
//
// Two stages, enough to show both halves of the trait. Replace them with your own; nothing below
// is required by the library.

/// Drops any message whose dotted body path does not equal an expected value.
///
/// A filter is the simplest useful stage: it returns nothing, and the message stops there.
pub struct FieldEquals {
    pub path: String,
    pub value: serde_json::Value,
}

impl Processor for FieldEquals {
    fn process(&mut self, m: ProcMsg) -> Out {
        match pluck(&m.msg.body, &self.path) {
            Some(v) if *v == self.value => smallvec::smallvec![m],
            _ => Out::new(),
        }
    }
}

/// Counts messages and emits a rollup on each tick.
///
/// This is the stateful half of the trait: it accumulates in `process` (emitting nothing) and
/// produces its output in `on_tick`. Windows, batches, and debounces are all this shape.
pub struct CountPerTick {
    pub seen: u64,
    pub last: Option<ProcMsg>,
}

impl Processor for CountPerTick {
    fn process(&mut self, m: ProcMsg) -> Out {
        self.seen += 1;
        self.last = Some(m);
        Out::new() // nothing goes downstream on arrival — see on_tick
    }

    fn on_tick(&mut self, _now_ms: u64) -> Out {
        let (Some(mut m), n) = (self.last.take(), std::mem::take(&mut self.seen)) else {
            return Out::new();
        };
        if n == 0 {
            return Out::new();
        }
        m.msg.body = serde_json::json!({ "count": n, "last": m.msg.body });
        smallvec::smallvec![m]
    }
}

/// Resolve a dotted path (`body.signal.id` style, minus the leading `body.`) inside a JSON value.
#[must_use]
pub fn pluck<'a>(v: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    path.split('.').try_fold(v, |acc, seg| acc.get(seg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgecommons::messaging::MessageBuilder;
    use serde_json::json;

    fn msg(body: serde_json::Value) -> ProcMsg {
        ProcMsg::new("ecv1/gw/x/main/data/t", MessageBuilder::new("T", "1.0").payload(body).build())
    }

    #[test]
    fn a_filter_stage_drops_what_does_not_match() {
        let mut p = Pipeline::new(vec![Box::new(FieldEquals {
            path: "quality".into(),
            value: json!("GOOD"),
        })]);
        let kept = p.run(smallvec::smallvec![msg(json!({ "quality": "GOOD" }))], None);
        assert_eq!(kept.len(), 1);

        let dropped = p.run(smallvec::smallvec![msg(json!({ "quality": "BAD" }))], None);
        assert!(dropped.is_empty(), "a filter that does not match emits nothing");
    }

    #[test]
    fn a_stateful_stage_emits_on_the_tick_not_on_arrival() {
        let mut p = Pipeline::new(vec![Box::new(CountPerTick { seen: 0, last: None })]);

        // Three messages arrive: nothing goes downstream yet.
        for _ in 0..3 {
            assert!(p.run(smallvec::smallvec![msg(json!({ "v": 1 }))], None).is_empty());
        }
        // The tick closes the window and emits one rollup.
        let out = p.run(Out::new(), Some(1_000));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].msg.body["count"], 3);

        // A second tick with nothing accumulated emits nothing — an empty window is not an event.
        assert!(p.run(Out::new(), Some(2_000)).is_empty());
    }

    #[test]
    fn stages_chain_and_a_tick_flows_through_the_rest_of_the_pipeline() {
        // Filter, then count. A window closing in stage 2 is emitted on the same pass.
        let mut p = Pipeline::new(vec![
            Box::new(FieldEquals { path: "quality".into(), value: json!("GOOD") }),
            Box::new(CountPerTick { seen: 0, last: None }),
        ]);
        p.run(smallvec::smallvec![msg(json!({ "quality": "GOOD" }))], None);
        p.run(smallvec::smallvec![msg(json!({ "quality": "BAD" }))], None); // filtered out
        let out = p.run(Out::new(), Some(1_000));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].msg.body["count"], 1, "only the GOOD message reached the counter");
    }

    #[test]
    fn pluck_walks_a_dotted_path() {
        let v = json!({ "signal": { "id": "temp-1" } });
        assert_eq!(pluck(&v, "signal.id").unwrap(), "temp-1");
        assert!(pluck(&v, "signal.nope").is_none());
    }
}
