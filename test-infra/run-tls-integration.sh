#!/usr/bin/env bash
# Run the secure-connection (TLS) integration tests for all three GGCommons
# libraries against the shared local EMQX broker.
#
# Steps it performs:
#   1. ensures the TLS test certs exist (runs gen-tls-certs.sh if missing)
#   2. brings the broker up (docker compose up -d) and waits for the :8883 listener
#   3. runs each library's gated TLS integration test with the right env vars
#
# Toolchains (python, cargo, mvn) must be on PATH. Override repo locations with
# GGCOMMONS_LIBS_DIR (defaults to this repo's parent directory). Each library's
# test self-skips if its toolchain or the broker is unavailable, so partial setups
# still report cleanly.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LIBS="${GGCOMMONS_LIBS_DIR:-$(cd "$HERE/.." && pwd)}"
CERTS="$HERE/tls-certs"

# Shared cert location (Python + Java) and the Rust test's individual-file vars.
export GGCOMMONS_TLS_CERTS_DIR="$CERTS"
export GGCOMMONS_IT_MQTT=1
export GGCOMMONS_IT_MQTT_CA="$CERTS/ca.crt"
export GGCOMMONS_IT_MQTT_CERT="$CERTS/client.crt"
export GGCOMMONS_IT_MQTT_KEY="$CERTS/client.key"

[ -f "$CERTS/ca.crt" ] || bash "$HERE/gen-tls-certs.sh"

echo "== bringing up broker =="
docker compose -f "$HERE/compose.yaml" up -d

echo "== waiting for TLS listener on :8883 =="
for _ in $(seq 1 30); do
  if MSYS_NO_PATHCONV=1 openssl s_client -connect localhost:8883 \
       -CAfile "$CERTS/ca.crt" -cert "$CERTS/client.crt" -key "$CERTS/client.key" \
       </dev/null >/dev/null 2>&1; then
    echo "broker TLS ready"; break
  fi
  sleep 2
done

rc=0
run() { echo; echo "===== $1 ====="; shift; ( "$@" ); local r=$?; [ $r -ne 0 ] && rc=1; return 0; }

run "Python" bash -c "cd '$LIBS/libs/python' && python -m pytest -m integration tests/test_tls_integration.py -v -rs"
run "Rust"   bash -c "cd '$LIBS/libs/rust' && cargo test --test tls_mqtt -- --nocapture"
run "Java"   bash -c "cd '$LIBS/libs/java' && mvn -q -Dtest=StandaloneTlsIntegrationTest -DfailIfNoTests=false test"

echo; echo "===== done (broker left running; 'docker compose -f $HERE/compose.yaml down' to stop) ====="
exit $rc
