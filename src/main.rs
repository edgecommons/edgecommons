//! # Rust Component Skeleton — entry point
//!
//! **One-liner purpose**: A worked-example AWS IoT Greengrass v2 component built on
//! the `ggcommons` Rust library, mirroring the Java and Python skeletons.
//!
//! ## Overview
//! Initializes [`ggcommons`] from the standard CLI contract (`-c`/`-m`/`-t`), then
//! hands control to [`app::SkeletonApp`], which demonstrates the library's
//! messaging, configuration, metrics, and heartbeat features. The component runs
//! until it receives a shutdown signal (Ctrl-C / SIGTERM), at which point dropping
//! the [`ggcommons::GgCommons`] runtime releases all resources (RAII).
//!
//! ## Running locally (STANDALONE mode, against a local MQTT broker)
//! ```bash
//! cargo run -- \
//!   -m STANDALONE ./test-configs/standalone-messaging.json \
//!   -c FILE ./test-configs/config.json \
//!   -t my-thing
//! ```
//!
//! ## Related Modules
//! - [`app`] holds the component's business logic.

mod app;

use ggcommons::prelude::*;

/// The component's full name (matches `recipe.yaml` / `gdk-config.json`).
const COMPONENT_NAME: &str = "aws.proserve.greengrass.RustComponentSkeleton";

/// Boot the component: build the runtime from CLI args, run the app, shut down cleanly.
///
/// # Purpose
/// Provide the `#[tokio::main]` entry that wires `ggcommons` and runs the demo app,
/// returning a process exit status via `anyhow::Result`.
///
/// # Errors
/// Propagates any error from runtime construction (bad CLI args, unreadable config,
/// failed broker connection) or from the app's run loop.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let gg = GgCommonsBuilder::new(COMPONENT_NAME)
        .args(std::env::args_os())
        .build()
        .await?;

    tracing::info!(
        component = gg.component_name(),
        thing = %gg.config().thing_name,
        "Rust Component Skeleton starting"
    );

    let app = app::SkeletonApp::new(&gg)?;
    app.run().await?;

    tracing::info!("Rust Component Skeleton stopped");
    Ok(())
}
