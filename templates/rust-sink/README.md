# <<COMPONENTNAME>>

A **sink component**: the last thing standing between data and its destination. It consumes work,
delivers it outward, and only then lets go of the source.

```text
  consume ──► deliver (idempotent, stable key) ──► verify ──► confirm ──► report
                       ▲                                                   │
                       └────────── retry with full jitter ◄────────────────┘
```

## Run it

```bash
cargo run -- \
  --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json \
  -t my-thing
```

The shipped example consumes every `data` message on the bus and writes each one to `./out`.

## Where your code goes

`src/dest.rs`. A backend implements `Destination`:

```rust
#[async_trait]
pub trait Destination: Send + Sync {
    fn kind(&self) -> &'static str;
    async fn deliver(&self, item: &Item) -> Result<Delivered>;
    async fn verify(&self, item: &Item, delivered: &Delivered) -> Result<()>;
}
```

`LocalDestination` ships as an example: it writes a temp file and **atomically renames** it into
place, so a reader never observes a half-written object and a crash leaves no corrupt artifact at
the real key. Add S3, HTTP, or whatever you are delivering to — everything above the trait (retry,
verification, reporting) is written against it and never learns what a bucket is.

## The order is the archetype

**Deliver idempotently, to a stable key.** The same item always lands in the same place, so a
redelivery is an *overwrite*, not a duplicate. A sink that cannot retry without duplicating cannot
retry at all.

**Verify before you confirm.** Trusting `deliver`'s `Ok` and releasing the source without checking
what actually landed is how you end up having deleted the only copy.

**Classify the failure.** `DeliverError::Transient` may succeed next time; `Permanent` never will.
Retrying a permanent failure burns the budget and floods the log; giving up on a transient one loses
data that a second attempt would have delivered.

**Report every transition.** `delivery-started` → `delivery-completed`, or `delivery-failed`
(carrying `willRetry`), and finally `delivery-exhausted` at Critical. A sink that fails quietly is
indistinguishable from one that is idle.

## Configuration

```json
{
  "id": "archive",
  "subscribe": "ecv1/+/+/+/data/#",
  "destination": { "type": "local", "path": "./out" },
  "retry": { "baseDelayMs": 1000, "maxDelayMs": 900000, "giveUpAfterMs": 3600000 }
}
```

The backoff is exponential with **full jitter**: without it, every component that lost the same
endpoint retries on the same instant, and an endpoint that is already struggling gets a synchronized
thundering herd on every boundary.

`giveUpAfterMs` is a **time budget, not an attempt count**. "Twenty attempts" means something very
different at 1 s and at 15 min of backoff; "keep trying for an hour" means the same thing at every
cadence, and it is what an operator can actually reason about.

`config.schema.json` is the contract, and `edgecommons component validate` checks against it —
including the config your Kubernetes ConfigMap and Greengrass recipe deploy with.
