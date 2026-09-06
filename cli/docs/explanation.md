# Explanation — why the tool is shaped this way

Background reading. None of this is needed to use the CLI, but it explains decisions that otherwise
look arbitrary — and it tells you what the tool will *never* do, which is often the more useful half.

## One static binary that carries its own templates

The CLI generates components in four languages, and it needs none of their toolchains to do it.
Scaffolding a Java component requires no JVM; scaffolding a TypeScript one requires no Node. You need
a toolchain to **build** what was generated, not to generate it.

That is possible because the templates and the canonical config schema are compiled *into* the
binary. The consequence worth noticing is that `component new` and `component validate` have no
network dependency and no registry dependency: they behave identically on a laptop, in CI, and on a
disconnected machine. A tool that quietly fetched a template would be a tool that stops working when
the network does, and edge work happens in places where the network does stop working.

## Local deployment inputs and explicit external tools

The deployment kernel reads local definitions, layers, bindings, and locks. Network-dependent work
is in adapters and external tools:

- **`registry` and `deployment lock`** read the registry through authenticated `gh` by default.
  Both accept a local catalog path for offline use. The current lock adapter resolves catalog
  metadata but does not yet resolve per-version digests or schemas from release indexes.
- **`component new --template-git`** clones a template from a URL instead of using the embedded one.
  It is opt-in: without that flag, scaffolding touches nothing.

- **`component package`** invokes GDK for Greengrass; build tools may download dependencies.
  `--publish` additionally invokes `gdk component publish` with the runner's credentials.

Config validation, deployment rendering and planning, local draft authoring, and release descriptor
generation do not need a network service. Building the CLI or a generated component can still need
dependency downloads.

## Validation is three layers, because mistakes come in three kinds

A JSON Schema catches a malformed document. It cannot catch `--transport IPC` on a HOST platform,
because both halves are individually valid. And neither catches a `Permissions:` block that AWS will
reject at `CreateComponentVersion` time.

So validation is layered: **schema** (`EC1xxx`), **semantic rules** (`EC2xxx`), and **artifact lint**
(`EC3xxx`). Each layer catches what the one below it structurally cannot. The third is the one people
find surprising and then come to rely on, because it converts a class of deploy-time failures into
commit-time ones.

The corollary is the `--platform` flag. Some semantic rules are only decidable with a target in hand.
Rather than guess a platform and produce confident-sounding wrong answers, the tool **skips those
rules and gives you less coverage** when you omit it. Knowing which rules did not run is more useful
than a false all-clear.

## The CLI produces; the runner publishes

`component release` describes existing artifacts, computes their digests, and writes a release descriptor. It does not
tag, upload, or publish. `component package --publish` shells out to `gdk` — deliberately, because
that keeps cloud SDKs out of the binary entirely.

The release workflow must supply provenance, signatures, and publication evidence; the current
descriptor leaves those fields empty. Deployment rendering and release-file generation do not apply
changes to devices. Local draft commands write Git branches, while review and merge remain the Git
host's responsibility.

## Config and artifacts are two streams, not one

A component's binary and its configuration are versioned, released, and rolled back **independently**.
`deployment release --stream config|artifact` promotes one at a time; the release lock records what
was in effect together without forcing them to move together.

Fusing them is the obvious-looking design and it is wrong: it means you cannot change a
configuration value without reshipping a binary, and cannot roll back a bad config without also
reverting a good binary. The release lock is a **correlation envelope**, not an atomic apply unit —
it answers "what config and which binary were live together" while leaving each free to move.

## Restart impact is derived, never assumed

When `plan` says a change restarts a component, that is computed from the component's **config
source**, not guessed from the platform. A watched file, a Kubernetes ConfigMap, a catalog push, and
a shadow update are all picked up live; an environment-variable change requires a new process; a
Greengrass `configurationUpdate` does not reliably restart anything either.

So hot-reload is a property of the source, not of the platform — and an operator sees the blast
radius before applying, rather than discovering it in production.

## Greengrass deploys per thing, never per thing group

Every member of a thing group shares one deployment document. IIoT edge devices each carry a unique
configuration, so a group cannot express per-device intent — the grouping primitive is simply wrong
for this fleet.

A definition's nodes therefore map one-to-one onto Greengrass deployments. N devices means N
deployments: more API calls, and in exchange, per-node partial failure and the staged, selectable
rollout you actually want.

## Recipes belong to the component, not to the deployment

A Greengrass recipe carries dependencies, access policies, lifecycle, platform, and default
configuration. Every one of those is a fact about the *component* — and every component repository
already authors exactly that, correctly, beside its GDK config.

So `deployment render --target GREENGRASS` produces deployment documents, not recipes. Per-device
configuration rides `configurationUpdate`, which overrides the recipe's defaults, so a deployment
never needs a bespoke recipe to carry site-specific config. Producing recipes is release
engineering's job, which is where the information already lives.

## Deterministic rendering is a build gate

Rendering the same definition twice produces byte-identical output: stable key order, LF endings, no
timestamps or hostnames in rendered artifacts — those belong in the release manifest, where they
describe the release rather than contaminating the thing being compared.

This is enforced by golden-file suites rather than asserted in prose. It is what makes drift
detection meaningful: if the bytes differ, something really changed.

## No aliases, and noun-verb naming

The surface is `component new`, not `create-component`. There are deliberately **no aliases** for the
older flat names. The CLI was never published, so the rename was free exactly once — and carrying
both `validate` and `component validate` forever would be a permanent trap in which the wrong one is
always one keystroke away.
