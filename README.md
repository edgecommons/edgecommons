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

## Convention
- TLS is keyed on the CA: a client with only `caPath` does **server-only** TLS;
  adding `certPath`+`keyPath` does **mutual** TLS. The broker above requires a client
  cert (mutual), so server-only clients are rejected — which is the intended check.
- These certs are for local testing only and expire in 10 years; re-run
  `gen-tls-certs.sh` to regenerate.
