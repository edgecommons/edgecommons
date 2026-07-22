# How-to Guides

> This documents the generated scaffold; rewrite it as you build the component out.

Recipes for specific tasks. Each assumes you already have the scaffold building and running (see the
[tutorial](tutorial.md)). For the concepts behind these steps, see [explanation.md](explanation.md).

---

## Add a destination

**Goal:** deliver to something other than the local filesystem (S3, SFTP, an HTTP endpoint, …).

1. Implement `Destination` — `kind()`, `deliver(item)` (must be **idempotent to `item.key()`** — the
   same item always lands at the same place, so a redelivery overwrites rather than duplicates), and
   `verify(item, delivered)` (must actually check what landed; `deliver` returning without throwing is
   not evidence).
2. Classify every failure your backend can raise as transient (worth retrying) or permanent (never
   will succeed) via `DeliverException`. When genuinely unsure, prefer transient — a wrongly-transient
   failure wastes retries; a wrongly-permanent one loses data.
3. Add a `case` to `Destination.build` for your backend's `type` tag.
4. Add a matching branch (`oneOf` variant) to `config.schema.json`'s `$defs/destination`.

`LocalDestination` is the reference to model against: it writes to a temp file and **renames it into
place** — a rename within a filesystem is atomic, so a reader never observes a half-written object,
and a crash mid-write leaves no corrupt artifact at the real key.

---

## Tune the retry policy

**Goal:** control how hard and how long a failed delivery is retried before it is reported exhausted.

```jsonc
"retry": { "baseDelayMs": 1000, "maxDelayMs": 900000, "giveUpAfterMs": 3600000 }
```

Backoff is exponential with **full jitter** (drawn uniformly from `[0, window)`) — not decoration:
without it, every component that lost the same endpoint retries at the same instant, and an endpoint
that is already struggling gets a synchronized thundering herd on every backoff boundary.
`giveUpAfterMs` is a **time budget**, not an attempt count — "keep trying for an hour" means the same
thing at every backoff cadence, which is what an operator can actually reason about.

---

## Replace the subscription source with a directory watch or a polled API

**Goal:** feed the sink from something other than a bus subscription.

This scaffold's source is a subscription in `run()`. Replace that call with your own source — a
watched directory, a polled API — and construct an `Item` (payload + stable key) for each unit of
work. Everything downstream of `deliverWithRetry` is unchanged; that is the point of the seam.

---

## Watch for lost data

**Goal:** be alerted the moment a delivery is truly lost, not just slow.

Subscribe `ecv1/+/+/+/evt/critical/#` — `delivery-exhausted` is the only critical event this archetype
raises, and it fires exactly when data did not arrive (a permanent failure, or the retry time budget
spent). `delivery-failed` (warning) is a transient hiccup that is still being retried; do not alert on
it the same way.

---

## Deploy to a platform

**Goal:** run the component on HOST, Greengrass, or Kubernetes.

**HOST (Docker / bare host):**
```bash
java -jar target/<<JARNAME>>-1.0.0.jar --platform HOST --transport MQTT ./messaging.json \
  -c FILE ./config.json -t my-thing
```
Or containerized, with a throw-away broker on the compose network: `docker compose up --build`.

**Greengrass:** the recipe's default destination is under the component's Greengrass work directory
(writable by `ggc_user`, not replaced by a deployment). Point it elsewhere and grant that user write
access to the new path.
```bash
gdk component build
gdk component publish
```

**Kubernetes:** the pod runs with a read-only root filesystem, so a `local` destination needs a
writable volume mounted at its path (`emptyDir` for a scratch demo; a PVC if the data must outlive
the pod).
```bash
docker build -t ghcr.io/<owner>/<<COMPONENTNAME>>:latest .
docker push ghcr.io/<owner>/<<COMPONENTNAME>>:latest
kubectl apply -f k8s/
```

## Build against the unreleased library (local development)

```bash
cd ../core/libs/java && mvn install -DskipTests
mvn -Dedgecommons.version=<the version that install just printed> package
```
