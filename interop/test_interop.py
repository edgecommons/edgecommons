"""Cross-language interoperability test for the ggcommons libraries.

Runs a request/reply round-trip over the shared local MQTT broker for every
ordered pair of languages (python, java, rust) using the per-language "interop
node" programs in this directory. A passing pair proves the message envelope and
the request/reply (reply_to + correlation_id) convention are mutually intelligible
in BOTH directions between those two libraries (request serialized by one,
deserialized + replied by the other, reply deserialized back by the first).

Prereqs (each self-skips if missing):
- a local MQTT broker on localhost:1883 (docker start ggcommons-emqx)
- python: the ggcommons package importable
- java:  a built shaded jar in ggcommons-java-lib/target + a JDK (JAVA_HOME or
         C:/Users/breis/tools/jdk), compiled by this module's fixture
- rust:  cargo available; the rust_node is built by this module's fixture

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
LANGS = ["python", "java", "rust"]


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
    jars = sorted((WORKSPACE / "ggcommons-java-lib" / "target").glob("ggcommons-*-shaded.jar"))
    return str(jars[-1]) if jars else None


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

    # Rust: cargo build the node.
    if shutil.which("cargo"):
        rn = HERE / "rust_node"
        try:
            subprocess.run(["cargo", "build"], cwd=rn, check=True, capture_output=True, timeout=600)
            exe = rn / "target" / "debug" / ("interop-rust-node.exe" if os.name == "nt"
                                             else "interop-rust-node")
            if exe.exists():
                cmd["rust"] = lambda *a, _e=str(exe): [_e, *a]
        except Exception:
            pass

    # Java: compile the node against the shaded jar.
    java, javac = _find_java()
    jar = _shaded_jar()
    if java and javac and jar:
        out = HERE / "java_node" / "out"
        out.mkdir(parents=True, exist_ok=True)
        try:
            subprocess.run([javac, "-cp", jar, "-d", str(out),
                            str(HERE / "java_node" / "InteropNode.java")],
                           check=True, capture_output=True, timeout=120)
            cp = jar + os.pathsep + str(out)
            cmd["java"] = lambda *a, _cp=cp, _j=java: [_j, "-cp", _cp, "InteropNode", *a]
        except Exception:
            pass

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
    finally:
        resp_proc.terminate()
        try:
            resp_proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            resp_proc.kill()
