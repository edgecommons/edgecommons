# Rust on-device validation

`examples/shadow_verify.rs` + `com.mbreissi.edgecommons.RustShadowVerify-1.0.0.yaml` validate
the **sanitized default SHADOW name** end-to-end on a live Greengrass nucleus: run as
`shadow_verify -c SHADOW` (no explicit name), the SHADOW source defaults the shadow
name to the component name and sanitizes it
(`com.mbreissi.edgecommons.RustShadowVerify` → `com_mbreissi_edgecommons_RustShadowVerify`), then loads the
config from that named shadow via ShadowManager IPC. A successful load of marker
values set in the cloud shadow under the sanitized name proves the
default→sanitize→`GetThingShadow` path runs.

## Reproduce

```bash
# 1. Cross-build for the device (Linux x64) — the greengrass feature is Linux-only.
#    From WSL (cargo at ~/.cargo/bin):
source ~/.cargo/env
cd libs/rust
CARGO_TARGET_DIR=/tmp/ggc-rust-target \
  cargo build --release --example shadow_verify --no-default-features --features greengrass
# binary: /tmp/ggc-rust-target/release/examples/shadow_verify

# 2. ShadowManager for the sanitized named shadow (device-local), set the cloud shadow:
sudo greengrass-cli deployment create --merge "aws.greengrass.ShadowManager=2.3.14" \
  --update-config '{"aws.greengrass.ShadowManager":{"MERGE":{"synchronize":{"coreThing":{"classic":true,"namedShadows":["com_mbreissi_edgecommons_RustShadowVerify"]},"direction":"betweenDeviceAndCloud"}}}}'
aws iot-data update-thing-shadow --thing-name <thing> --shadow-name com_mbreissi_edgecommons_RustShadowVerify \
  --cli-binary-format raw-in-base64-out --payload file://desired.json out.json
# desired.json: {"state":{"desired":{"ComponentConfig":"<stringified edgecommons config>"}}}

# 3. Deploy the component (raw-binary artifact at <artifactDir>/<name>/1.0.0/shadow_verify):
sudo greengrass-cli deployment create --recipeDir recipes --artifactDir artifacts \
  --merge "com.mbreissi.edgecommons.RustShadowVerify=1.0.0"

# 4. Result -> /tmp/rust_shadow_verify_result.json  (expect config_source SHADOW + the marker values)
```

Result observed: `{"config_source":"SHADOW","config_loaded":{"publish_interval":23.0,
"site":"rust-shadow",...},"connected":true,"lang":"rust"}`. See the broader TS
validation matrix in `../../ts/deploy/VALIDATION.md`.
