//! # <<COMPONENTNAME>> — entry point
//!
//! An AWS IoT Greengrass v2 component built on the `ggcommons` Rust library.
//! Initializes the runtime from the standard CLI contract (`-c`/`--platform`/`--transport`/`-t`),
//! then hands control to [`app::App`]. The component runs until a shutdown signal
//! (Ctrl-C / SIGTERM); dropping the [`ggcommons::GgCommons`] runtime then releases
//! all resources (RAII).
//!
//! ## Running locally (HOST platform, MQTT transport, against a local MQTT broker)
//! ```bash
//! cargo run -- \
//!   --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
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
        identity = %gg.config().identity().path(),
        "<<COMPONENTNAME>> starting"
    );

    let app = app::App::new(&gg)?;
    app.run(&gg).await?;

    tracing::info!("<<COMPONENTNAME>> stopped");
    Ok(())
}
