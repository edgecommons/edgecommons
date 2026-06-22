# Cross-language interoperability tests

Proves the four ggcommons libraries (Python, Java, Rust, TypeScript) interoperate over
MQTT: the message envelope and the request/reply convention (`reply_to` topic +
`correlation_id`) are mutually intelligible across languages.

## How it works

Each language ships a small dual-role **interop node** that talks to the shared
local MQTT broker in STANDALONE local-only mode:

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

Nodes:
- `python_node.py` — uses the installed `ggcommons` package.
- `rust_node/` — a small cargo binary depending on `libs/rust` by path.
- `java_node/InteropNode.java` — compiled against the java lib's shaded jar.
- the **TypeScript** node lives in `libs/ts` (`src/interop_node.ts`) and is compiled to
  `dist/interop_node.js` by the test fixture (`npm install` + `npm run build`) — that is
  why there is no `ts_node/` directory here.

## Running

```bash
docker start ggcommons-emqx           # local broker on :1883
# build the java shaded jar once: (in libs/java) mvn -DskipTests package
python -m pytest interop/test_interop.py -v
```

The test self-skips any language whose toolchain/artifact is missing (no cargo,
no JDK/shaded jar, no node/npm, or `ggcommons` not importable), and skips entirely
if no broker is reachable. The Java jar, Rust binary, and TypeScript node are built
by the test's fixtures; `java -cp`, `cargo`, and `node`/`npm` toolchains are
auto-discovered (JAVA_HOME or `C:/Users/breis/tools/jdk`).
