# On-device GREENGRASS-mode IPC verification

Deploys the TS `IpcProvider` (via `src/ipc_verify.ts`) as a one-shot Greengrass
component to prove it interoperates over **Greengrass IPC** with a live nucleus and
with the other ggcommons libraries.

`ipc_verify.ts` runs three checks against the nucleus and writes the result as JSON
to `/tmp/ts_ipc_verify_result.json` (and stdout / the component log):

1. **request/reply** over IPC (correlation id round-trips, body echoed),
2. **raw** publish/ingest over IPC,
3. **cross-language** — ingests the heartbeat envelope published over IPC by a
   peer ggcommons component (e.g. the deployed Java skeleton on
   `ggcommons/<thing>/<ComponentName>/heartbeat`).

## Prerequisites

- A device running an AWS IoT Greengrass v2 nucleus with `greengrass-cli`.
- Node.js (≥18) on the device (`apt-get install -y nodejs npm`).
- A peer ggcommons component publishing heartbeats over IPC (for check 3).

## Reproduce

```bash
# 1. Build the lib ON THE DEVICE (so aws-crt resolves the device's native binary)
#    Copy package.json, tsconfig.json and src/ to the device, then:
npm install && npm run build

# 2. Stage the artifact: zip with dist/, node_modules/, package.json at the ROOT
mkdir -p stage/tsverify && cp -r dist node_modules package.json stage/tsverify/
( cd stage/tsverify && zip -rq ../../tsverify.zip dist node_modules package.json )

# 3. Lay out recipe + artifact for a local deployment
mkdir -p ggc/recipes ggc/artifacts/com.ggcommons.TsIpcVerify/1.0.2
cp com.ggcommons.TsIpcVerify-1.0.2.yaml ggc/recipes/
cp tsverify.zip ggc/artifacts/com.ggcommons.TsIpcVerify/1.0.2/

# 4. Deploy
sudo /greengrass/v2/bin/greengrass-cli deployment create \
  --recipeDir   ggc/recipes \
  --artifactDir ggc/artifacts \
  --merge "com.ggcommons.TsIpcVerify=1.0.2"

# 5. Read the result (component runs as root → /tmp file is world-readable)
cat /tmp/ts_ipc_verify_result.json    # expect "all_ok": true

# 6. Clean up
sudo /greengrass/v2/bin/greengrass-cli deployment create \
  --remove "com.ggcommons.TsIpcVerify"
```

## Notes

- The recipe runs with `RequiresPrivilege: true` (root), so the nucleus passes the
  IPC env directly. To run **non-root** (`ggc_user`), the device must let
  privilege-dropping `sudo -E` preserve the IPC env vars — on distros that ship
  `sudo-rs` as the default `sudo` (no `-E`), switch to classic sudo and add a
  `Defaults setenv` drop-in (the same caveat documented for the Rust port).
- The artifact zip's **root** must contain `dist/`, `node_modules/`, `package.json`
  (do not zip the parent folder, or the unarchived path gains an extra level).
