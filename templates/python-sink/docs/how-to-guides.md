# How-to Guides

*This documents the generated scaffold; rewrite it as you build the component out.*

Recipes for specific tasks. Each assumes the scaffold runs (see the [tutorial](tutorial.md)). For
concepts see [explanation.md](explanation.md); for exhaustive options see [reference/](reference/).

---

## Write a new destination

Implement `Destination` (`app/dest.py`): `kind()`, `deliver(item) -> Delivered`,
`verify(item, delivered) -> None`. Two properties are non-negotiable, whatever the backend:

- **Deliver to a deterministic, stable key.** The same item must always land at the same place, so a
  redelivery is an idempotent overwrite, not a duplicate — that's what makes retry safe.
- **Verify before the source is released.** Check that what actually landed matches what you sent —
  releasing the source because `deliver` merely *returned* is how you lose the only copy.

```python
class S3Destination(Destination):
    def kind(self) -> str:
        return "s3"

    def deliver(self, item: Item) -> Delivered:
        try:
            self._client.put_object(Bucket=self._bucket, Key=item.key, Body=item.data)
        except ClientError as e:
            raise DeliverError.transient_failure(str(e))  # or permanent_failure, per the error code
        return Delivered(len(item.data))

    def verify(self, item: Item, delivered: Delivered) -> None:
        head = self._client.head_object(Bucket=self._bucket, Key=item.key)
        if head["ContentLength"] != delivered.bytes_written:
            raise DeliverError.transient_failure("size mismatch")
```

Add a branch to `build_destination()` **and** a matching `oneOf` variant to `config.schema.json`'s
`destination` definition — the two are one contract.

## Classify failures correctly

Getting `transient` wrong is expensive in both directions: retrying a permanent failure (bad
credentials, a malformed key) burns the retry budget and floods the log; giving up on a transient one
(a timeout, a 503, a full disk someone will empty) loses data a second attempt would have delivered.
When genuinely unsure, prefer transient — a wrongly-transient failure wastes retries; a
wrongly-permanent one loses data outright.

## Add a sink

Each entry of `component.instances[]` is one sink — its own subscription, its own destination, its
own retry policy:

```jsonc
{
  "id": "audit",
  "subscribe": "ecv1/+/+/+/evt/#",
  "destination": { "type": "local", "path": "./audit" },
  "retry": { "baseDelayMs": 500, "giveUpAfterMs": 1800000 }
}
```

`id` is the sink's UNS instance token **and** the prefix of every key it writes — keep it stable;
changing it sends every future redelivery somewhere new.

## Tune the retry policy

`baseDelayMs` / `maxDelayMs` / `giveUpAfterMs` set the exponential-backoff-with-full-jitter shape and
the time budget. The give-up is a **time budget, not an attempt count** — "keep trying for an hour"
means the same thing at every backoff cadence, which an attempt count does not. Raise
`giveUpAfterMs` for a destination you expect to have longer outages; lower it for one where stale
data is worse than lost data.

## Report a real connection

A sink's destinations **are** its instances — `instance_connectivity()` already returns one entry per
configured destination, driven by `DestinationHealth`. You do not need to add anything here unless you
add a destination type whose health needs richer `attributes` than `{"destination": kind}`.

## Deploy to a platform

**HOST:** `python3 main.py --platform HOST --transport MQTT ./messaging.json -c FILE ./config.json -t my-thing`
(or `docker compose up --build`).

**Greengrass:** `gdk component build && gdk component publish`. The recipe's default configuration
ships one working sink delivering into the component's writable work dir — because a sink with no
instances has nothing to deliver and refuses to start. Set a real S3 bucket in `gdk-config.json`
first — a scaffold with no bucket configured carries a visible sentinel that `component validate`
treats as an error.

**Kubernetes:** build `./Dockerfile`, push or `kind load` it, set `image:` in `k8s/deployment.yaml`,
then `kubectl apply -f k8s/`. The scaffold's sink delivers into an **emptyDir**, which dies with the
pod — fine for a first run, wrong for a sink you rely on. Point it at a PersistentVolumeClaim or a
destination that isn't this pod's disk before you trust it with data. With `--platform auto` the
library detects KUBERNETES, reads config from the mounted ConfigMap (hot-reloaded on `kubectl
apply`), and resolves identity from the Downward API.

## Wire up CI

`.github/workflows/ci.yml` calls the org's reusable component-CI workflow plus a `coverage` job
enforcing the 90% line-coverage gate; `.github/workflows/deploy-docs.yml` refreshes the docs site on
doc-only pushes once this component is registered in `registry/components.json`. Both are inert until
pushed to GitHub with the org secrets configured.
