# Cross-language interoperability tests

Proves the four edgecommons libraries (Python, Java, Rust, TypeScript) interoperate over
MQTT: the message envelope and the request/reply convention (`reply_to` topic +
`correlation_id`) are mutually intelligible across languages.

## Mandatory scope

This suite is the required validation gate for any core change that changes or enhances
on-the-wire behavior or structure. That includes the message envelope, body encodings
or markers, headers, request/reply semantics, raw-message conventions, UNS topics and
classes, reserved-topic handling, and config options that change what the libraries emit
or accept on MQTT. Extend all four interop nodes and the matrix assertions in the same
change; per-language unit tests alone do not prove interoperability.

## How it works

Each language ships a small dual-role **interop node** that talks to the shared
local MQTT broker over a local-only MQTT transport:

- `responder <topic>` — subscribe to `<topic>`; reply to each request with
  `{"echo": <request body>, "responder": "<lang>"}` (the lib copies the
  correlation id and publishes to the request's `reply_to`). Prints `READY` once
  subscribed.
- `request <topic> <token>` — send `{"token": <token>, "from": "<lang>"}`, wait
  for the reply, print one JSON line, exit 0 on a correlated, well-formed reply.

`test_interop.py` runs both a request/reply round-trip and a raw publish/ingest
round-trip for **every ordered pair** of the four languages (4×4×2 = 32 combos). A
passing pair exercises serialization in *both* directions (request serialized by the
requester + parsed/replied by the responder; reply parsed back by the requester).

## UNS roles (M14 — UNS-CANONICAL-DESIGN §7)

Each node additionally implements three UNS roles over its library's real UNS surface:

- `uns-pub <identityJson> <class> [channel]` — parse the wire-form identity with the
  lib's lenient parser, mint the topic with the real `uns()` builder
  (includeRoot=false), build a message stamped with that identity via the real
  message builder, publish it, and print one JSON line
  `{"ok": true, "topic": <topic>, "envelope": <wire JSON>}`.
- `uns-sub <topic>` — subscribe (prints `READY`), receive one envelope, and print
  `{"ok": <identity parsed>, "identity": <identity|null>, "body": <body>}`.
- `uns-guard` — attempt a raw publish to the reserved-class topic
  `ecv1/dev1/comp1/main/state` through the guarded public surface; exits NON-ZERO
  printing the reserved-topic error name (Java `ReservedTopicException`, Python/TS
  `ReservedTopicError`, Rust `EdgeCommonsError::ReservedTopic`).

`test_uns_topic_parity` (4×4 publisher×subscriber pairs) asserts every language mints
the **byte-identical** topic from a fixed identity and that the receiver parses a
**structurally identical** top-level `identity` (D-U22); `test_uns_guard` asserts the
reserved-class guard rejects in all four languages (D-U24).

Nodes (each consumes its library's public API, like a real component):
- `python_node.py` — uses the installed `edgecommons` package.
- `rust_node/` — a small cargo binary depending on `libs/rust` by path.
- `java_node/InteropNode.java` — compiled against the java lib's shaded jar.
- `ts_node/` — a small TypeScript package depending on `edgecommons` (the `libs/ts`
  npm package); resolved through the repo npm workspace and compiled to
  `ts_node/dist/interop_node.js` by the test fixture.

## Running

```bash
docker start edgecommons-emqx           # local broker on :1883
# build the java shaded jar once: (in libs/java) mvn -DskipTests package
python -m pytest interop/test_interop.py -v
```

The test self-skips any language whose toolchain/artifact is missing (no cargo,
no JDK/shaded jar, no node/npm, or `edgecommons` not importable), and skips entirely
if no broker is reachable. The Java jar, Rust binary, and TypeScript node are built
by the test's fixtures; `java -cp`, `cargo`, and `node`/`npm` toolchains are
auto-discovered (JAVA_HOME or `C:/Users/breis/tools/jdk`).
