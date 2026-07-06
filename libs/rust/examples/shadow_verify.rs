//! On-device verification (Rust) for the sanitized default SHADOW name.
//!
//! Deployed as a Greengrass component run as `shadow_verify -c SHADOW` (no explicit
//! name): the SHADOW config source defaults the shadow name to the component name
//! and sanitizes it (`com.mbreissi.edgecommons.RustShadowVerify` -> `com_mbreissi_edgecommons_RustShadowVerify`).
//! It reads the loaded config and writes a JSON result to `/tmp` so the loaded values
//! (set in the cloud named shadow under the sanitized name) prove the default→sanitize
//! →GetThingShadow path runs end-to-end. Build with `--features greengrass` on Linux.

use std::fs;
use std::time::Duration;

use edgecommons::prelude::*;
use serde_json::json;

const COMPONENT_NAME: &str = "com.mbreissi.edgecommons.RustShadowVerify";
const RESULT: &str = "/tmp/rust_shadow_verify_result.json";

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        let _ = fs::write(
            RESULT,
            format!(
                "{}\n",
                json!({ "lang": "rust", "connected": false, "error": e.to_string() })
            ),
        );
        std::process::exit(1);
    }
}

async fn run() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let gg = EdgeCommonsBuilder::new(COMPONENT_NAME)
        .args(std::env::args_os())
        .build()
        .await?;

    let cfg = gg.config();
    let source = match &gg.args().config {
        ConfigSourceSpec::Shadow { .. } => "SHADOW",
        ConfigSourceSpec::Greengrass { .. } => "GG_CONFIG",
        ConfigSourceSpec::File { .. } => "FILE",
        ConfigSourceSpec::ConfigMap { .. } => "CONFIGMAP",
        ConfigSourceSpec::Env { .. } => "ENV",
        ConfigSourceSpec::ConfigComponent => "CONFIG_COMPONENT",
    };
    let publish_interval = cfg
        .global()
        .get("publish_interval")
        .and_then(|v| v.as_f64());
    let site = cfg.parsed.tags.get("site").and_then(|v| v.as_str());

    let out = json!({
        "lang": "rust",
        "connected": true,
        "config_source": source,
        "config_loaded": {
            "publish_interval": publish_interval,
            "site": site,
            "thing": cfg.thing_name,
        }
    });
    fs::write(RESULT, format!("{}\n", out))?;

    // Stay RUNNING briefly, then exit; dropping `gg` releases resources (RAII).
    tokio::time::sleep(Duration::from_secs(20)).await;
    Ok(())
}
