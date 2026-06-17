//! # <<COMPONENTNAME>> — entry point
//!
//! An AWS IoT Greengrass v2 component built on the `ggcommons` Rust library.
//! Initializes the runtime from the standard CLI contract (`-c`/`-m`/`-t`), then
//! hands control to [`app::App`]. The component runs until a shutdown signal
//! (Ctrl-C / SIGTERM); dropping the [`ggcommons::GgCommons`] runtime then releases
//! all resources (RAII).
//!
//! ## Running locally (STANDALONE mode, against a local MQTT broker)
//! ```bash
//! cargo run -- \
//!   -m STANDALONE ./test-configs/standalone-messaging.json \
//!   -c FILE ./test-configs/config.json \
//!   -t my-thing
//! ```

mod app;

use ggcommons::prelude::*;

/// The component's full name (matches `recipe.yaml` / `gdk-config.json`).
const COMPONENT_NAME: &str = "<<COMPONENTFULLNAME>>";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let gg = GgCommonsBuilder::new(COMPONENT_NAME)
        .args(std::env::args_os())
        .build()
        .await?;

    tracing::info!(
        component = gg.component_name(),
        thing = %gg.config().thing_name,
        "<<COMPONENTNAME>> starting"
    );

    let app = app::App::new(&gg)?;
    app.run().await?;

    tracing::info!("<<COMPONENTNAME>> stopped");
    Ok(())
}
