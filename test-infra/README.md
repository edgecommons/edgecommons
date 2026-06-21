# ggcommons-test-infra

Shared integration-test infrastructure for the GGCommons libraries
(`ggcommons-java-lib`, `ggcommons-python-lib`, `ggcommons-rust-lib`). It provides a
local MQTT broker (EMQX) — plaintext **and** TLS — so each library's standalone /
secure-connection integration tests run against the same broker setup.

## Prerequisites
- Docker (Docker Desktop on Windows)
- `bash` + `openssl` (Git Bash on Windows) for cert generation

## 1. Generate the TLS test certificates (once)
```bash
bash gen-tls-certs.sh
```
Creates `./tls-certs/` with a throwaway CA, a server cert (CN/SAN `localhost`), and a
client cert — all signed by the CA. `./tls-certs/` is gitignored; never commit keys.

## 2. Start the broker
```bash
docker compose up -d        # from this directory
docker compose down         # stop + remove
```
Ports: `1883` (plaintext MQTT), `8883` (TLS, **mutual** by default), `18083` (dashboard).
Container name: `ggcommons-emqx`.

For a **server-only** TLS listener (client cert not required), delete the `VERIFY`
and `FAIL_IF_NO_PEER_CERT` lines in `compose.yaml`.

## 3. Point the libraries' tests at the certs
Integration tests locate the certs via the **`GGCOMMONS_TLS_CERTS_DIR`** environment
variable (absolute path to this repo's `tls-certs/`). Example (Python):
```bash
GGCOMMONS_TLS_CERTS_DIR=/abs/path/to/ggcommons-test-infra/tls-certs \
  python -m pytest -m integration tests/test_tls_integration.py
```
If the variable is unset, a library may fall back to a local `tests/tls-certs/`
directory; tests skip cleanly when no certs / no broker are available.

## Running the libraries' secure-connection tests

One command (brings up the broker, generates certs if needed, runs all three):
```bash
bash run-tls-integration.sh
```

Or per library (broker up + `GGCOMMONS_TLS_CERTS_DIR` exported first):
```bash
# Python — locates certs via GGCOMMONS_TLS_CERTS_DIR
GGCOMMONS_TLS_CERTS_DIR=$PWD/tls-certs \
  python -m pytest -m integration tests/test_tls_integration.py -v        # in ggcommons-python-lib

# Java — same env var; self-skips (mvn verify stays green) when it is unset
GGCOMMONS_TLS_CERTS_DIR=$PWD/tls-certs \
  mvn -Dtest=StandaloneTlsIntegrationTest -DfailIfNoTests=false test       # in ggcommons-java-lib

# Rust — gated on GGCOMMONS_IT_MQTT=1 + per-file cert vars
GGCOMMONS_IT_MQTT=1 GGCOMMONS_IT_MQTT_CA=$PWD/tls-certs/ca.crt \
  GGCOMMONS_IT_MQTT_CERT=$PWD/tls-certs/client.crt \
  GGCOMMONS_IT_MQTT_KEY=$PWD/tls-certs/client.key \
  cargo test --test tls_mqtt -- --nocapture                               # in ggcommons-rust-lib
```
All three skip cleanly when the broker/certs are absent, so they never break a
normal build.

## Convention
- TLS is keyed on the CA: a client with only `caPath` does **server-only** TLS;
  adding `certPath`+`keyPath` does **mutual** TLS. The broker above requires a client
  cert (mutual), so server-only clients are rejected — which is the intended check.
- These certs are for local testing only and expire in 10 years; re-run
  `gen-tls-certs.sh` to regenerate.

## Deployed-component integration test (`test_deployed_component.py`)

Closes the coverage gap that let several deploy-path bugs through: the per-language
suites run STANDALONE/MQTT with the `log` metric target and never exercise GREENGRASS
IPC, the deployed-config flow, the CloudWatch target, or the vault under the GG work
dir. This test runs **on a Greengrass core device** against a live nucleus and asserts
each deployed ggcommons component:

- reached `State: RUNNING` (the deploy resolved — artifact staged, recipe valid, config
  schema-accepted, IPC connected as `ggc_user`, no crash-loop), and
- shows **ongoing GG IPC messaging** in its current log, plus **encrypted-vault
  credential access** (`credential access OK`) across retained logs.

It would have failed on every bug from this work (BROKEN component from the metric
crash / messaging NPE / vault PermissionError / IPC connect timeout, or a `ggc_user`
component that only ran with `RequiresPrivilege`).

```bash
# On the core device (uses the local greengrass-cli + /greengrass/v2/logs; needs sudo).
GGCOMMONS_IT_GG=1 python3 test_deployed_component.py
# or via pytest (skips unless GGCOMMONS_IT_GG=1):
GGCOMMONS_IT_GG=1 python3 -m pytest test_deployed_component.py -v
# Override the component set (default = the four skeleton examples):
GGCOMMONS_IT_COMPONENTS="com.a,com.b" GGCOMMONS_IT_GG=1 python3 test_deployed_component.py
```
Verified 2026-06-21 against the lab nucleus (lab-5950x): 4/4 skeletons healthy.
