"""Cross-language interoperability test for the ggcommons libraries.

Runs a request/reply round-trip over the shared local MQTT broker for every
ordered pair of languages (python, java, rust, ts) using the per-language "interop
node" programs (the ts node is compiled inside libs/ts to dist/interop_node.js). A passing pair proves the message envelope and
the request/reply (reply_to + correlation_id) convention are mutually intelligible
in BOTH directions between those two libraries (request serialized by one,
deserialized + replied by the other, reply deserialized back by the first).

Prereqs (each self-skips if missing):
- a local MQTT broker on localhost:1883 (docker start ggcommons-emqx)
- python: the ggcommons package importable
- java:  a built shaded jar in ggcommons-java-lib/target + a JDK (JAVA_HOME or
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
WORKSPACE = HERE.parent.parent  # .../source/ggcommons
HOST = os.environ.get("GGCOMMONS_IT_MQTT_HOST", "localhost")
PORT = int(os.environ.get("GGCOMMONS_IT_MQTT_PORT", "1883"))
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
    jars = [j for j in target.glob("ggcommons-*.jar")
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
        subprocess.run([sys.executable, "-c", "import ggcommons"], check=True,
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

    # TypeScript: install the npm workspace from the repo root (so the ggcommons
    # lib and the ts_node interop package link cleanly), then build the ggcommons
    # lib (ts_node imports its public API) and the ts_node package itself. (Absent
    # node/npm -> skip; a build *failure* raises so a broken node surfaces loudly
    # instead of skipping.)
    node, npm = shutil.which("node"), shutil.which("npm")
    if node and npm:
        # npm is a .cmd shim on Windows; run via shell there.
        r = subprocess.run(f'"{npm}" install', cwd=WORKSPACE, capture_output=True, text=True,
                           timeout=600, shell=True)
        assert r.returncode == 0, f"ts npm install failed:\n{r.stderr}"
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
            f'"{npm}" run build --workspace=@edgecommons/ggcommons --workspace=ggcommons-interop-ts-node',
            cwd=WORKSPACE, capture_output=True, text=True, timeout=300, shell=True)
        assert r.returncode == 0, f"ts build failed:\n{r.stderr}\n{r.stdout}"
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


@pytest.mark.parametrize("subscriber", LANGS)
@pytest.mark.parametrize("publisher", LANGS)
def test_interop_raw_publish(commands, publisher, subscriber):
    """One language publishes a raw (non-envelope) payload; another ingests it as a
    raw message. Proves the {"raw": <value>} wire convention interoperates."""
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

        # The subscriber exits after receiving (or its own 10s timeout).
        try:
            sub_proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            pass

        payload = _last_json(lines)
        assert payload is not None, f"no JSON from {subscriber} raw-sub; lines={lines}"
        assert payload["ok"] is True, f"{publisher}->{subscriber} raw failed: {payload}"
        assert payload["is_raw"] is True, "non-envelope payload must arrive as a raw message"
        assert payload["raw_token"] == token, "raw payload must round-trip intact"
    finally:
        if sub_proc.poll() is None:
            sub_proc.terminate()
            try:
                sub_proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                sub_proc.kill()
