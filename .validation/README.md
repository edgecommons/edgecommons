# Credentials Phase 3 — real-AWS lab validation

One-off validation that the edgecommons credentials subsystem syncs from **real AWS Secrets
Manager** over the lab core device's **real TES role** (the leg never exercised before; KMS-via-TES
was already covered against the floci emulator). Scope chosen: **Secrets Manager only** (no KMS CMK,
so teardown is immediate and complete).

- Account `162499689067`, region `us-east-1`, core device `lab-5950x`, TES role
  `GreengrassV2TokenExchangeRole`.
- All created resources are listed in `manifest.json`.

## Scripts (run from this folder, Windows AWS creds = IAM user `marc`)
- `setup.ps1` — create the secret + attach the scoped inline TES policy (`tes-policy.json`).
- `teardown.ps1` — delete both (idempotent). **Run this when done.**

## Validate from the lab (after setup)
SSH to `marc@192.168.1.229`, fetch real TES creds via the device cert, and run a central sync:

```bash
CREDS=$(sudo curl -s --cert /greengrass/v2/thingCert.crt --key /greengrass/v2/privKey.key \
  --cacert /greengrass/v2/rootCA.pem \
  https://c2x01woqd17shc.credentials.iot.us-east-1.amazonaws.com/role-aliases/GreengrassV2TokenExchangeRoleAlias/credentials)
export AWS_ACCESS_KEY_ID=$(echo "$CREDS" | python3 -c 'import sys,json;print(json.load(sys.stdin)["credentials"]["accessKeyId"])')
export AWS_SECRET_ACCESS_KEY=$(echo "$CREDS" | python3 -c 'import sys,json;print(json.load(sys.stdin)["credentials"]["secretAccessKey"])')
export AWS_SESSION_TOKEN=$(echo "$CREDS" | python3 -c 'import sys,json;print(json.load(sys.stdin)["credentials"]["sessionToken"])')
export AWS_REGION=us-east-1 PYTHONPATH=/tmp/ggpkg   # /tmp/ggpkg = copy of libs/python/edgecommons with an empty __init__.py
# then: open_from_config({"central":{"type":"awsSecretsManager","region":"us-east-1","sync":{"secrets":["db/password"]}}, "vault":{"path":...}}, "lab-5950x/edgecommons-cred-validation")
# get_string("db/password") == "validation-secret-v1"
```

The namespace `lab-5950x/edgecommons-cred-validation` + sync secret `db/password` maps to the central
id `lab-5950x/edgecommons-cred-validation/db/password` (the auto-namespaced default), which is the
secret created by `setup.ps1`.

## Result (2026-06-21)
✅ Device fetched the secret from real Secrets Manager over real TES; credential service synced
`validation-secret-v1` into the local vault. Resources then torn down (`teardown.ps1`).
