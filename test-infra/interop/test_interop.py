"""Cross-language interoperability test for the edgecommons libraries.

Runs request/reply, deferred command outcomes, QoS1 confirmed publication,
raw-publish drop policy, opaque binary body, and UNS round-trips over the
shared local MQTT broker for every ordered pair of languages (python, java,
rust, ts) using the per-language "interop node" programs. A passing pair
proves normal protobuf messages are mutually intelligible in both directions
and that raw/foreign payloads do not leak through normal Message subscriptions.

Prereqs (each self-skips if missing):
- a local MQTT broker on localhost:1883 (docker start edgecommons-emqx)
- python: the edgecommons package importable
- java:  a built shaded jar in edgecommons-java-lib/target + a JDK (JAVA_HOME or
         C:/Users/breis/tools/jdk), compiled by this module's fixture
- rust:  cargo available; the rust_node is built by this module's fixture
- ts:    node + npm available; libs/ts is npm-installed + tsc-built by this fixture

Run:  python -m pytest interop/test_interop.py -v
"""
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import uuid
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
# Run the node subprocesses here so the Java Paho client's file-persistence
# directories land in a temp dir, not in the repo.
RUN_DIR = Path(tempfile.mkdtemp(prefix="ggc-interop-"))
WORKSPACE = HERE.parent.parent  # .../source/edgecommons
HOST = os.environ.get("EDGECOMMONS_IT_MQTT_HOST", "localhost")
PORT = int(os.environ.get("EDGECOMMONS_IT_MQTT_PORT", "1883"))
LANGS = ["python", "java", "rust", "ts"]

# Canonical payload permutations every requester sends as the request body's `types` field; the
# responder echoes it. A deep round-trip across all 16 ordered pairs proves cross-language payload
# fidelity (serialized by the requester, parsed + re-serialized by the responder, parsed back).
# Both a null inside an array AND a top-level null-valued MAP entry (`nullv`) are tested: since #15,
# the Java sender preserves null-valued Map entries (serializeNulls for Map payloads), so all four
# languages now round-trip explicit nulls as JSON null.
EXPECTED_TYPES = {
    "b": True, "bf": False,
    "i": 42, "ni": -7, "fl": 3.5,
    "slash": "a/b", "quote": 'x"y',
    "arr": [1, "two", False, None],
    "nullv": None,
    "nested": {"k": [1, {"d": 2}]},
    "ea": [], "eo": {},
}
BINARY_BODY_HEX = "000102030a0d1f207f80feff"


def _payload_eq(exp, act):
    """Deep payload equality across languages: bool-strict, number-lenient (Java's Gson parses every
    JSON number to a double, so 42 and 42.0 must compare equal), structure-strict otherwise."""
    if isinstance(exp, bool) or isinstance(act, bool):
        return exp is act
    if isinstance(exp, (int, float)) and isinstance(act, (int, float)):
        return float(exp) == float(act)
    if isinstance(exp, dict) and isinstance(act, dict):
        return exp.keys() == act.keys() and all(_payload_eq(exp[k], act[k]) for k in exp)
    if isinstance(exp, list) and isinstance(act, list):
        return len(exp) == len(act) and all(_payload_eq(x, y) for x, y in zip(exp, act))
    return exp == act


def _broker_up():
    try:
        with socket.create_connection((HOST, PORT), timeout=2):
            return True
    except OSError:
        return False


pytestmark = pytest.mark.skipif(not _broker_up(), reason="no MQTT broker on localhost:1883")


# --- toolchain / artifact discovery -------------------------------------------------

def _find_java():
    for cand in (os.environ.get("JAVA_HOME"), r"C:\Users\breis\tools\jdk"):
        if cand:
            exe = Path(cand) / "bin" / ("java.exe" if os.name == "nt" else "java")
            jc = Path(cand) / "bin" / ("javac.exe" if os.name == "nt" else "javac")
            if exe.exists() and jc.exists():
                return str(exe), str(jc)
    j, jc = shutil.which("java"), shutil.which("javac")
    return (j, jc) if j and jc else (None, None)


def _shaded_jar():
    # `mvn package` shades into the MAIN jar (replace mode: `original-*.jar` is the
    # pre-shade thin jar). Select the dependency-bearing jar, preferring an explicit
    # `-shaded` classifier if a build ever attaches one.
    target = WORKSPACE / "libs" / "java" / "target"
    jars = [j for j in target.glob("edgecommons-*.jar")
            if not j.name.startswith("original-")
            and not j.name.endswith(("-sources.jar", "-javadoc.jar", "-shaded.jar"))]
    # Pick the most-recently-built jar (by mtime), NOT the name-sorted last: a stale higher-version
    # jar left in target/ (e.g. a pre-rename 1.3.2-SNAPSHOT) would otherwise name-sort above the
    # current 0.1.0 build and silently run interop against an old library.
    return str(max(jars, key=lambda p: p.stat().st_mtime)) if jars else None


# Built once and reused; populated by the session fixtures below.
_CMD = {}


@pytest.fixture(scope="session")
def commands():
    """Build each node once and return a dict: lang -> function(role, *args) -> argv list.
    Languages whose toolchain/artifact is unavailable are omitted (combos skip)."""
    cmd = {}

    # Python: needs the package importable.
    try:
        subprocess.run([sys.executable, "-c", "import edgecommons"], check=True,
                       capture_output=True, timeout=60)
        py_node = str(HERE / "python_node.py")
        cmd["python"] = lambda *a: [sys.executable, py_node, *a]
    except Exception:
        pass

    # Rust: cargo build the node. (Absent cargo -> skip; a build *failure* raises,
    # so a broken node surfaces loudly instead of masquerading as a skip.)
    if shutil.which("cargo"):
        rn = HERE / "rust_node"
        r = subprocess.run(["cargo", "build"], cwd=rn, capture_output=True, text=True, timeout=600)
        assert r.returncode == 0, f"rust node build failed:\n{r.stderr}"
        exe = rn / "target" / "debug" / ("interop-rust-node.exe" if os.name == "nt"
                                         else "interop-rust-node")
        if exe.exists():
            cmd["rust"] = lambda *a, _e=str(exe): [_e, *a]

    # TypeScript: install the npm workspace from the repo root (so the edgecommons
    # lib and the ts_node interop package link cleanly), then build the edgecommons
    # lib (ts_node imports its public API) and the ts_node package itself. (Absent
    # node/npm -> skip; a build *failure* raises so a broken node surfaces loudly
    # instead of skipping.)
    node, npm = shutil.which("node"), shutil.which("npm")
    if node and npm:
        # A root workspace install hoists TypeScript to node_modules/.bin; older local
        # installs may still have a package-local libs/ts/node_modules/.bin. Support both.
        bin_name = "tsc.cmd" if os.name == "nt" else "tsc"
        ts_bins = [
            WORKSPACE / "node_modules" / ".bin",
            WORKSPACE / "libs" / "ts" / "node_modules" / ".bin",
        ]
        tsc = next((bin_dir / bin_name for bin_dir in ts_bins if (bin_dir / bin_name).exists()),
                   ts_bins[0] / bin_name)
        if not tsc.exists():
            # npm is a .cmd shim on Windows; run via shell there.
            r = subprocess.run(f'"{npm}" install', cwd=WORKSPACE, capture_output=True, text=True,
                               timeout=600, shell=True)
            assert r.returncode == 0, f"ts npm install failed:\n{r.stderr}"
            tsc = next((bin_dir / bin_name for bin_dir in ts_bins if (bin_dir / bin_name).exists()),
                       ts_bins[0] / bin_name)
            assert tsc.exists(), f"tsc not found after npm install; checked: {ts_bins}"
        env = os.environ.copy()
        env["PATH"] = os.pathsep.join(str(p) for p in ts_bins) + os.pathsep + env.get("PATH", "")
        # #14 guard: force-clean the TS build outputs so the ts_node can never silently run against a
        # stale libs/ts build (the "old library" trap that made the ts raw-publisher time out even
        # though publishRaw was correct). The plain `tsc` build below then regenerates both dists from
        # current source, so ts_node always links the freshly-built library.
        for stale_dist in (WORKSPACE / "libs" / "ts" / "dist", HERE / "ts_node" / "dist"):
            shutil.rmtree(stale_dist, ignore_errors=True)
        for ts_dir in (WORKSPACE / "libs" / "ts", HERE / "ts_node"):
            for info in ts_dir.glob("*.tsbuildinfo"):
                info.unlink(missing_ok=True)
        r = subprocess.run(
            f'"{npm}" run build --workspace=@edgecommons/edgecommons',
            cwd=WORKSPACE, env=env, capture_output=True, text=True, timeout=300, shell=True)
        assert r.returncode == 0, f"ts lib build failed:\n{r.stderr}\n{r.stdout}"
        r = subprocess.run([str(tsc), "-p", str(HERE / "ts_node" / "tsconfig.json")],
                           cwd=WORKSPACE, env=env, capture_output=True, text=True, timeout=300)
        assert r.returncode == 0, f"ts node build failed:\n{r.stderr}\n{r.stdout}"
        node_js = HERE / "ts_node" / "dist" / "interop_node.js"
        if node_js.exists():
            cmd["ts"] = lambda *a, _n=node, _js=str(node_js): [_n, _js, *a]

    # Java: compile the node against the shaded jar.
    java, javac = _find_java()
    jar = _shaded_jar()
    if java and javac and jar:
        out = HERE / "java_node" / "out"
        out.mkdir(parents=True, exist_ok=True)
        r = subprocess.run([javac, "-cp", jar, "-d", str(out),
                            str(HERE / "java_node" / "InteropNode.java")],
                           capture_output=True, text=True, timeout=120)
        assert r.returncode == 0, f"java node compile failed:\n{r.stderr}"
        cp = jar + os.pathsep + str(out)
        cmd["java"] = lambda *a, _cp=cp, _j=java: [_j, "-cp", _cp, "InteropNode", *a]

    _CMD.update(cmd)
    return cmd


def _wait_ready(proc, timeout=20.0):
    """Block until the responder prints READY (draining stdout in a thread)."""
    ready = threading.Event()

    def reader():
        for line in proc.stdout:
            if "READY" in line:
                ready.set()
        # keep draining to EOF so the child never blocks on a full pipe

    threading.Thread(target=reader, daemon=True).start()
    return ready.wait(timeout)


def _launch(cmd):
    """Start a node, capturing all stdout lines (for nodes that print READY and then
    a result line). Returns (proc, lines, ready_event)."""
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                            text=True, cwd=str(RUN_DIR))
    lines, ready = [], threading.Event()

    def reader():
        for line in proc.stdout:
            lines.append(line)
            if "READY" in line:
                ready.set()

    threading.Thread(target=reader, daemon=True).start()
    return proc, lines, ready


def _last_json(lines):
    for ln in reversed(lines):
        s = ln.strip()
        if s.startswith("{"):
            return json.loads(s)
    return None


@pytest.mark.parametrize("responder", LANGS)
@pytest.mark.parametrize("requester", LANGS)
def test_interop_request_reply(commands, requester, responder):
    for lang in (requester, responder):
        if lang not in commands:
            pytest.skip(f"{lang} toolchain/artifact unavailable")

    topic = f"interop/{responder}/{uuid.uuid4()}"
    token = uuid.uuid4().hex

    resp_proc = subprocess.Popen(
        commands[responder]("responder", topic),
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, cwd=str(RUN_DIR),
    )
    try:
        assert _wait_ready(resp_proc), f"{responder} responder never signalled READY"

        result = subprocess.run(
            commands[requester]("request", topic, token),
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=30,
            cwd=str(RUN_DIR),
        )
        # The requester prints a single JSON line as its last stdout line.
        last = [ln for ln in result.stdout.splitlines() if ln.strip().startswith("{")][-1]
        payload = json.loads(last)

        assert result.returncode == 0, f"{requester}->{responder} requester failed: {result.stdout}\n{result.stderr}"
        assert payload["ok"] is True
        assert payload["correlation_match"] is True, "correlation id must round-trip"
        body = payload["reply_body"]
        assert body["responder"] == responder, f"reply should come from the {responder} responder"
        assert body["echo"]["token"] == token, "responder must echo the request body"
        # Payload-permutation fidelity: the canonical `types` object must survive the full round-trip
        # (requester serialize -> responder parse + re-serialize -> requester parse) for every pair.
        echoed_types = body["echo"].get("types")
        assert echoed_types is not None, f"{requester}->{responder}: request 'types' missing from echo"
        assert _payload_eq(EXPECTED_TYPES, echoed_types), (
            f"payload permutations must round-trip {requester}->{responder}; got {echoed_types}"
        )
    finally:
        resp_proc.terminate()
        try:
            resp_proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            resp_proc.kill()


@pytest.mark.parametrize("responder", LANGS)
@pytest.mark.parametrize("requester", LANGS)
def test_interop_deferred_command(commands, requester, responder):
    """A real command inbox accepts work before activation, then sends exactly one
    confirmed deferred reply with the original correlation id.  Every ordered pair
    validates requester parsing as well as responder settlement over EMQX."""
    for lang in (requester, responder):
        if lang not in commands:
            pytest.skip(f"{lang} toolchain/artifact unavailable")

    component = f"interop-deferred-{responder}-{uuid.uuid4().hex}"
    topic = f"ecv1/interop-device/{component}/main/cmd/deferred"
    token = uuid.uuid4().hex
    resp_proc, lines, ready = _launch(commands[responder]("deferred-responder", component))
    try:
        assert ready.wait(25), f"{responder} deferred responder never signalled READY; lines={lines}"
        result = subprocess.run(
            commands[requester]("deferred-request", topic, token),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=30,
            cwd=str(RUN_DIR),
        )
        payload = _last_json(result.stdout.splitlines())
        assert result.returncode == 0, (
            f"{requester}->{responder} deferred request failed: {result.stdout}\n{result.stderr}"
        )
        assert payload is not None, f"no JSON from {requester} deferred request: {result.stdout}"
        assert payload["ok"] is True
        assert payload["reply_count"] == 1, "one deferred command must settle exactly once"
        assert payload["correlation_match"] is True, "deferred reply must preserve request correlation"
        body = payload["reply_body"]
        assert body["ok"] is True
        result_body = body["result"]
        assert result_body["token"] == token
        assert result_body["responder"] == responder
        assert result_body["durablyAccepted"] is True
    finally:
        if resp_proc.poll() is None:
            resp_proc.terminate()
            try:
                resp_proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                resp_proc.kill()


@pytest.mark.parametrize("subscriber", LANGS)
@pytest.mark.parametrize("publisher", LANGS)
def test_interop_confirmed_qos1_publish(commands, publisher, subscriber):
    """A publisher reports success only after its strict public QoS1 path returns;
    the receiver keeps a duplicate-detection window and rejects a second envelope."""
    for lang in (publisher, subscriber):
        if lang not in commands:
            pytest.skip(f"{lang} toolchain/artifact unavailable")

    topic = f"interop/confirmed/{subscriber}/{uuid.uuid4()}"
    token = uuid.uuid4().hex
    sub_proc, lines, ready = _launch(commands[subscriber]("confirmed-sub", topic, token))
    try:
        assert ready.wait(25), f"{subscriber} confirmed subscriber never signalled READY; lines={lines}"
        published = subprocess.run(
            commands[publisher]("confirmed-pub", topic, token),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=30,
            cwd=str(RUN_DIR),
        )
        pub_payload = _last_json(published.stdout.splitlines())
        assert published.returncode == 0, (
            f"{publisher} confirmed publish failed: {published.stdout}\n{published.stderr}"
        )
        assert pub_payload == {"ok": True, "confirmed": True, "qos": 1}, (
            "publisher may report success only after its strict QoS1 confirmation returns"
        )
        sub_proc.wait(timeout=15)
        payload = _last_json(lines)
        assert payload is not None, f"no JSON from {subscriber} confirmed subscriber; lines={lines}"
        assert payload["ok"] is True
        assert payload["message_count"] == 1, "QoS1 probe must not emit a duplicate envelope"
        assert payload["body"]["token"] == token
        assert payload["body"]["from"] == publisher
    finally:
        if sub_proc.poll() is None:
            sub_proc.terminate()
            try:
                sub_proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                sub_proc.kill()


@pytest.mark.parametrize("subscriber", LANGS)
@pytest.mark.parametrize("publisher", LANGS)
def test_interop_raw_publish(commands, publisher, subscriber):
    """One language publishes a raw/foreign payload; another subscribes through the
    normal Message path and must not receive it. Raw publish remains an explicit
    escape hatch, but normal subscriptions are protobuf-only."""
    for lang in (publisher, subscriber):
        if lang not in commands:
            pytest.skip(f"{lang} toolchain/artifact unavailable")

    topic = f"interop/raw/{subscriber}/{uuid.uuid4()}"
    token = uuid.uuid4().hex

    sub_proc, lines, ready = _launch(commands[subscriber]("raw-sub", topic, token))
    try:
        assert ready.wait(20), f"{subscriber} raw-sub never signalled READY"

        pub = subprocess.run(commands[publisher]("raw-pub", topic, token),
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                             timeout=30, cwd=str(RUN_DIR))
        assert pub.returncode == 0, f"{publisher} raw-pub failed: {pub.stdout}\n{pub.stderr}"

        sub_proc.wait(timeout=15)

        payload = _last_json(lines)
        assert payload is not None, f"no JSON from {subscriber} raw-sub; lines={lines}"
        assert payload["ok"] is True, f"{publisher}->{subscriber} raw failed: {payload}"
        assert payload["delivered"] is False, "raw payload must not arrive as a normal Message"
    finally:
        if sub_proc.poll() is None:
            sub_proc.terminate()
            try:
                sub_proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                sub_proc.kill()


@pytest.mark.parametrize("subscriber", LANGS)
@pytest.mark.parametrize("publisher", LANGS)
def test_interop_binary_body_publish(commands, publisher, subscriber):
    """One language publishes a first-class binary message body; another ingests and
    decodes it through its public binary body API. All ordered pairs prove exact-byte
    wire compatibility for the binary marker over the local MQTT transport."""
    for lang in (publisher, subscriber):
        if lang not in commands:
            pytest.skip(f"{lang} toolchain/artifact unavailable")

    topic = f"interop/binary/{subscriber}/{uuid.uuid4()}"

    sub_proc, lines, ready = _launch(commands[subscriber]("binary-sub", topic, BINARY_BODY_HEX))
    try:
        assert ready.wait(20), f"{subscriber} binary-sub never signalled READY"

        pub = subprocess.run(commands[publisher]("binary-pub", topic, BINARY_BODY_HEX),
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                             timeout=30, cwd=str(RUN_DIR))
        assert pub.returncode == 0, f"{publisher} binary-pub failed: {pub.stdout}\n{pub.stderr}"

        try:
            sub_proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            pass

        payload = _last_json(lines)
        assert payload is not None, f"no JSON from {subscriber} binary-sub; lines={lines}"
        assert payload["ok"] is True, f"{publisher}->{subscriber} binary failed: {payload}"
        assert payload["is_binary"] is True, "envelope body must be recognized as binary"
        assert payload["hex"] == BINARY_BODY_HEX, "binary body bytes must survive exactly"
    finally:
        if sub_proc.poll() is None:
            sub_proc.terminate()
            try:
                sub_proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                sub_proc.kill()


@pytest.mark.parametrize("subscriber", LANGS)
@pytest.mark.parametrize("publisher", LANGS)
def test_interop_typed_telemetry_byte_sample(commands, publisher, subscriber):
    """One language publishes a standard SouthboundSignalUpdate protobuf body with
    a byte-valued sample; another language decodes the typed body and verifies the
    nested sample bytes plus source/server timestamps."""
    for lang in (publisher, subscriber):
        if lang not in commands:
            pytest.skip(f"{lang} toolchain/artifact unavailable")

    topic = f"interop/typed/{subscriber}/{uuid.uuid4()}"

    sub_proc, lines, ready = _launch(commands[subscriber]("typed-sub", topic, BINARY_BODY_HEX))
    try:
        assert ready.wait(20), f"{subscriber} typed-sub never signalled READY"

        pub = subprocess.run(commands[publisher]("typed-pub", topic, BINARY_BODY_HEX),
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                             timeout=30, cwd=str(RUN_DIR))
        assert pub.returncode == 0, f"{publisher} typed-pub failed: {pub.stdout}\n{pub.stderr}"

        try:
            sub_proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            pass

        payload = _last_json(lines)
        assert payload is not None, f"no JSON from {subscriber} typed-sub; lines={lines}"
        assert payload["ok"] is True, f"{publisher}->{subscriber} typed telemetry failed: {payload}"
        assert payload["body_case"] == "SOUTHBOUND_SIGNAL_UPDATE"
        assert payload["hex"] == BINARY_BODY_HEX
        assert payload["source_ts_ms"] == 1783360799900
        assert payload["server_ts_ms"] == 1783360800000
    finally:
        if sub_proc.poll() is None:
            sub_proc.terminate()
            try:
                sub_proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                sub_proc.kill()


def _log_topic(publisher):
    return f"ecv1/interop-device/interop-log-{publisher}/main/log/warn"


def _identity_device(identity):
    hier = (identity or {}).get("hier") or []
    return hier[-1].get("value") if hier else None


@pytest.mark.parametrize("subscriber", LANGS)
@pytest.mark.parametrize("publisher", LANGS)
def test_interop_log_bus(commands, publisher, subscriber):
    """One language publishes a structured log record through its runtime log facade
    (`gg.logs()` / `getLogs()` / `logs()`), and another language receives the
    canonical UNS `log/{level}` envelope over the local MQTT transport."""
    for lang in (publisher, subscriber):
        if lang not in commands:
            pytest.skip(f"{lang} toolchain/artifact unavailable")

    token = uuid.uuid4().hex
    topic = _log_topic(publisher)

    sub_proc, lines, ready = _launch(commands[subscriber]("log-sub", topic, token))
    try:
        assert ready.wait(20), f"{subscriber} log-sub never signalled READY"

        pub = subprocess.run(
            commands[publisher]("log-pub", token),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=45,
            cwd=str(RUN_DIR),
        )
        assert pub.returncode == 0, f"{publisher} log-pub failed: {pub.stdout}\n{pub.stderr}"
        pub_out = _last_json(pub.stdout.splitlines())
        assert pub_out is not None, f"no JSON from {publisher} log-pub: {pub.stdout}"
        assert pub_out["ok"] is True, f"{publisher} log-pub did not publish: {pub_out}"
        assert pub_out["component"] == f"interop-log-{publisher}"

        try:
            sub_proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            pass

        payload = _last_json(lines)
        assert payload is not None, f"no JSON from {subscriber} log-sub; lines={lines}"
        assert payload["ok"] is True, f"{publisher}->{subscriber} log bus failed: {payload}"
        assert payload["topic"] == topic
        assert payload["header"]["name"] == "log"
        assert payload["header"]["version"] == "1.0"
        identity = payload["identity"]
        assert _identity_device(identity) == "interop-device"
        assert identity["component"] == f"interop-log-{publisher}"
        assert identity["instance"] == "main"
        body = payload["body"]
        assert body["schema"] == "edgecommons.log.v1"
        assert body["level"] == "WARN"
        assert body["logger"] == f"interop.{publisher}"
        assert body["message"] == f"log-interop-{token}"
        assert body["fields"]["nonce"] == token
        assert body["fields"]["publisher"] == publisher
    finally:
        if sub_proc.poll() is None:
            sub_proc.terminate()
            try:
                sub_proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                sub_proc.kill()


# --- UNS suite (M14 — UNS-CANONICAL-DESIGN §7, D-U22/D-U24) --------------------------

# The fixed conformance identity every language's `uns-pub` is handed (wire form). The
# topic below is what the real `uns()` builder must mint from it, byte-for-byte, in all
# four languages (includeRoot=false; instance defaults through the identity itself).
UNS_IDENTITY = {
    "hier": [
        {"level": "site", "value": "dallas"},
        {"level": "zone", "value": "zone-3"},
        {"level": "device", "value": "gw-01"},
    ],
    "path": "dallas/zone-3/gw-01",
    "component": "interop",
    "instance": "main",
}
UNS_CLASS = "data"
UNS_CHANNEL = "temp"
EXPECTED_UNS_TOPIC = "ecv1/gw-01/interop/main/data/temp"


@pytest.mark.parametrize("subscriber", LANGS)
@pytest.mark.parametrize("publisher", LANGS)
def test_uns_topic_parity(commands, publisher, subscriber):
    """Every language's `uns-pub` must mint the SAME topic byte-for-byte from the fixed
    identity, and a subscriber in any language must parse a structurally-identical
    top-level `identity` element out of the received envelope (D-U22: topics compare
    byte-for-byte, envelopes structurally)."""
    for lang in (publisher, subscriber):
        if lang not in commands:
            pytest.skip(f"{lang} toolchain/artifact unavailable")

    sub_proc, lines, ready = _launch(commands[subscriber]("uns-sub", EXPECTED_UNS_TOPIC))
    try:
        assert ready.wait(20), f"{subscriber} uns-sub never signalled READY"

        pub = subprocess.run(
            commands[publisher]("uns-pub", json.dumps(UNS_IDENTITY), UNS_CLASS, UNS_CHANNEL),
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=30,
            cwd=str(RUN_DIR))
        assert pub.returncode == 0, f"{publisher} uns-pub failed: {pub.stdout}\n{pub.stderr}"
        pub_out = _last_json(pub.stdout.splitlines())
        assert pub_out is not None, f"no JSON from {publisher} uns-pub: {pub.stdout}"
        assert pub_out["ok"] is True

        # Byte-identical topic across all four languages (each publisher is asserted
        # against the same pinned constant, so all pairs are transitively identical).
        assert pub_out["topic"] == EXPECTED_UNS_TOPIC, (
            f"{publisher} minted '{pub_out['topic']}', expected '{EXPECTED_UNS_TOPIC}'")

        # The sent envelope carries the top-level identity (structural equality; JSON
        # member order is not normative) and no tags.thing (hard cut).
        envelope = pub_out["envelope"]
        assert envelope.get("identity") == UNS_IDENTITY, (
            f"{publisher} envelope identity mismatch: {envelope.get('identity')}")
        assert "thing" not in (envelope.get("tags") or {}), "tags.thing must be gone"

        # The subscriber exits after receiving (or its own 10s timeout).
        try:
            sub_proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            pass

        received = _last_json(lines)
        assert received is not None, f"no JSON from {subscriber} uns-sub; lines={lines}"
        assert received["ok"] is True, f"{publisher}->{subscriber} uns failed: {received}"
        assert received["identity"] == UNS_IDENTITY, (
            f"{subscriber} parsed identity {received['identity']}, expected {UNS_IDENTITY}")
        assert received["body"]["from"] == publisher, "envelope body must name the publisher"
    finally:
        if sub_proc.poll() is None:
            sub_proc.terminate()
            try:
                sub_proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                sub_proc.kill()


@pytest.mark.parametrize("lang", LANGS)
def test_uns_guard(commands, lang):
    """Each language's `uns-guard` attempts a raw publish to the reserved-class topic
    ecv1/dev1/comp1/main/state through its guarded public surface and must exit
    NON-ZERO printing the reserved-topic error name (Java ReservedTopicException /
    Python+TS ReservedTopicError / Rust EdgeCommonsError::ReservedTopic — all carry the
    common 'ReservedTopic' stem)."""
    if lang not in commands:
        pytest.skip(f"{lang} toolchain/artifact unavailable")

    result = subprocess.run(commands[lang]("uns-guard"),
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                            timeout=30, cwd=str(RUN_DIR))
    assert result.returncode != 0, (
        f"{lang} uns-guard must exit non-zero (the guard must reject the reserved"
        f" topic): {result.stdout}\n{result.stderr}")
    payload = _last_json(result.stdout.splitlines())
    assert payload is not None, f"no JSON from {lang} uns-guard: {result.stdout}\n{result.stderr}"
    assert "ReservedTopic" in str(payload.get("error")), (
        f"{lang} uns-guard must name the reserved-topic error, got: {payload}")


# ---------------------------------------------------------------------------------------------
# `status` verb + `state.instances[]`  (the per-instance connectivity wire surface)
#
# Both surfaces are fed by ONE component-supplied provider: the state keepalive PUSHES the sample
# in `instances[]`, and the built-in `status` verb RETURNS it when pulled. So the two must agree,
# in every language, for every producer/consumer pair — which is exactly what these two matrices
# assert. A language that emitted `"state": null` instead of omitting it, or that dropped the open
# `attributes` bag, would look fine in its own unit tests and only fail here.
# ---------------------------------------------------------------------------------------------

# The canonical sample every node's provider must report, verbatim. Chosen to pin the contract:
#   cam-01  every optional member present; `attributes` carries an array, a string and a number,
#           so the OPEN bag is proven to survive a JSON round-trip across four languages.
#   cam-02  connected=false with a RICHER state — BACKOFF ("still trying") is not FAILED
#           ("gave up"), and a boolean cannot tell them apart. That is why `state` exists.
#   cam-03  the minimal element: NO state, NO detail, NO attributes. Optional members must be
#           OMITTED, never emitted as null/empty — this element is what catches that.
EXPECTED_INSTANCES = [
    {
        "instance": "cam-01",
        "connected": True,
        "state": "ONLINE",
        "detail": "rtsp://cam-01/stream",
        "attributes": {"capabilities": ["ptz", "snapshot"], "vendor": "acme", "retries": 0},
    },
    {"instance": "cam-02", "connected": False, "state": "BACKOFF", "detail": "connect timed out"},
    {"instance": "cam-03", "connected": True},
]


def _assert_instances(actual, producer, consumer):
    """The instances[] payload must survive producer -> consumer byte-for-byte in meaning."""
    assert actual is not None, f"{producer}->{consumer}: instances[] missing entirely"
    by_id = {e["instance"]: e for e in actual}
    assert set(by_id) == {"cam-01", "cam-02", "cam-03"}, (
        f"{producer}->{consumer}: expected exactly the 3 canonical instances, got {sorted(by_id)}")

    for expected in EXPECTED_INSTANCES:
        got = by_id[expected["instance"]]
        # `connected` is the NORMALIZED flag: always present, in every language, for every element.
        assert got["connected"] == expected["connected"], (
            f"{producer}->{consumer}: {expected['instance']} connected mismatch: {got}")
        for member in ("state", "detail", "attributes"):
            if member in expected:
                assert got.get(member) == expected[member], (
                    f"{producer}->{consumer}: {expected['instance']}.{member} did not round-trip: "
                    f"expected {expected[member]!r}, got {got.get(member)!r}")
            else:
                # Omission is the contract, not decoration: this rides a keepalive that ships every
                # 5s per component. A language emitting "state": null would pass its own tests and
                # fail here.
                assert member not in got, (
                    f"{producer}->{consumer}: {expected['instance']} must OMIT `{member}`, "
                    f"not emit it as {got.get(member)!r}")


@pytest.mark.parametrize("requester", LANGS)
@pytest.mark.parametrize("responder", LANGS)
def test_interop_status_verb(commands, responder, requester):
    """PULL: the built-in `status` verb returns the provider sample, cross-language (16 pairs)."""
    for lang in (requester, responder):
        if lang not in commands:
            pytest.skip(f"{lang} toolchain/artifact unavailable")

    component = f"statusresp{uuid.uuid4().hex[:8]}"

    resp = subprocess.Popen(
        commands[responder]("status-responder", component),
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, cwd=str(RUN_DIR),
    )
    try:
        assert _wait_ready(resp), f"{responder} status-responder never signalled READY"

        result = subprocess.run(
            commands[requester]("status-request", component),
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=40,
            cwd=str(RUN_DIR),
        )
        assert result.returncode == 0, (
            f"{requester}->{responder} status-request failed: {result.stdout}\n{result.stderr}")
        payload = _last_json(result.stdout.splitlines())
        assert payload is not None, f"no JSON from {requester}: {result.stdout}"
        assert payload["ok"] is True, f"{requester}->{responder}: status not ok: {payload}"

        body = payload["reply_body"]
        assert body["status"] == "RUNNING"
        assert isinstance(body["uptimeSecs"], int), "status is ping's superset: uptimeSecs required"
        _assert_instances(body.get("instances"), responder, requester)
    finally:
        resp.terminate()
        try:
            resp.wait(timeout=10)
        except subprocess.TimeoutExpired:
            resp.kill()


@pytest.mark.parametrize("subscriber", LANGS)
@pytest.mark.parametrize("publisher", LANGS)
def test_interop_state_instances(commands, publisher, subscriber):
    """PUSH: the `state` keepalive carries the same sample in instances[] (16 pairs)."""
    for lang in (publisher, subscriber):
        if lang not in commands:
            pytest.skip(f"{lang} toolchain/artifact unavailable")

    component = f"statepub{uuid.uuid4().hex[:8]}"

    # _launch (not _wait_ready + communicate): _wait_ready drains stdout to EOF on a daemon
    # thread, so a later communicate() on the same pipe returns ''. _launch keeps every line,
    # which is what the other subscriber tests use and what we need — the result JSON arrives
    # AFTER the READY line, on the same pipe.
    sub, lines, ready = _launch(commands[subscriber]("state-instances-sub", component))
    try:
        assert ready.wait(25), (
            f"{subscriber} state-instances-sub never signalled READY; lines={lines}")

        pub = subprocess.Popen(
            commands[publisher]("state-instances-pub", component),
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, text=True, cwd=str(RUN_DIR),
        )
        try:
            # The subscriber exits as soon as it sees a RUNNING state carrying instances[].
            sub.wait(timeout=45)
        finally:
            pub.terminate()
            try:
                pub.wait(timeout=10)
            except subprocess.TimeoutExpired:
                pub.kill()

        payload = _last_json(lines)
        assert payload is not None, f"{publisher}->{subscriber}: no state observed; lines={lines}"
        assert payload["ok"] is True, f"{publisher}->{subscriber}: {payload}"
        assert payload["state_status"] == "RUNNING", "instances[] rides the RUNNING keepalive only"
        _assert_instances(payload.get("instances"), publisher, subscriber)
    finally:
        if sub.poll() is None:
            sub.terminate()
            try:
                sub.wait(timeout=10)
            except subprocess.TimeoutExpired:
                sub.kill()
