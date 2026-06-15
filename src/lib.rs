//! # GGCommons (Rust)
//!
//! Rust implementation of the Greengrass Commons library — a third implementation
//! alongside the Java (canonical) and Python libraries. It bundles the
//! cross-cutting concerns of an AWS IoT Greengrass v2 component (configuration,
//! messaging, metrics, heartbeat, logging) behind service traits so component
//! authors write only business logic.
//!
//! **Status: Phase 0 scaffold** — the standalone-mode MVP is in progress. See
//! `../GGCOMMONS_RUST_PORT.md` for the full design and plan.
//!
//! ```no_run
//! use ggcommons::prelude::*;
//!
//! # async fn run() -> ggcommons::Result<()> {
//! let gg = GgCommonsBuilder::new("com.example.MyComponent")
//!     .args(std::env::args_os())
//!     .build()
//!     .await?;
//!
//! let cfg = gg.config();
//! println!("component {} on thing {}", gg.component_name(), cfg.thing_name);
//! # Ok(())
//! # }
//! ```

pub mod cli;
pub mod config;
pub mod error;
pub mod heartbeat;
pub mod logging;
pub mod messaging;
pub mod metrics;

pub use error::{GgError, Result};

use std::ffi::OsString;
use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::cli::ParsedArgs;
use crate::config::model::Config;

/// Default thing name when none is supplied and not running under Greengrass.
const DEFAULT_THING_NAME: &str = "NOT_GREENGRASS";
/// Greengrass-injected environment variable for the core's thing name.
const THING_NAME_ENV: &str = "AWS_IOT_THING_NAME";

/// The initialized component runtime. Holds the wired services and the current
/// configuration snapshot. Dropping it releases owned resources (RAII) — there is
/// no separate `close()` to forget.
pub struct GgCommons {
    component_name: String,
    args: ParsedArgs,
    config: Arc<ArcSwap<Config>>,
    messaging: Option<Arc<dyn messaging::MessagingService>>,
}

impl GgCommons {
    /// The component's full name.
    pub fn component_name(&self) -> &str {
        &self.component_name
    }

    /// The parsed standard CLI arguments.
    pub fn args(&self) -> &ParsedArgs {
        &self.args
    }

    /// A consistent snapshot of the current configuration. Cheap to call; returns
    /// the live snapshot, which is replaced atomically on reload (Phase 1).
    pub fn config(&self) -> Arc<Config> {
        self.config.load_full()
    }

    /// The messaging service for this component.
    ///
    /// # Purpose
    /// Obtain the wired [`messaging::MessagingService`] (the testable seam) for
    /// publish/subscribe and request/reply.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `GgError::Messaging` | Messaging is not available in this mode (GREENGRASS IPC messaging is Phase 2) | Run in STANDALONE mode, or wait for Phase 2 |
    pub fn messaging(&self) -> Result<Arc<dyn messaging::MessagingService>> {
        self.messaging.clone().ok_or_else(|| {
            GgError::Messaging(
                "messaging is not available in this mode (GREENGRASS IPC messaging is Phase 2)"
                    .to_string(),
            )
        })
    }
}

/// Fluent builder for [`GgCommons`] (the supported construction path).
pub struct GgCommonsBuilder {
    component_name: String,
    argv: Option<Vec<OsString>>,
}

impl GgCommonsBuilder {
    /// Start building a component runtime with the given full component name.
    pub fn new(component_name: impl Into<String>) -> Self {
        Self {
            component_name: component_name.into(),
            argv: None,
        }
    }

    /// Supply the argv (including the program name, as from `std::env::args_os()`).
    /// If not set, the process arguments are used.
    pub fn args<I, T>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString>,
    {
        self.argv = Some(args.into_iter().map(Into::into).collect());
        self
    }

    /// Parse arguments, load and validate configuration, initialize logging, and
    /// return the runtime.
    ///
    /// Phase 0 wires configuration + logging. Messaging, metrics, and heartbeat
    /// are wired in Phase 1.
    pub async fn build(self) -> Result<GgCommons> {
        let parsed = match self.argv {
            Some(argv) => cli::parse_from(argv)?,
            None => cli::parse_from(std::env::args_os())?,
        };

        let thing_name = parsed.thing.clone().unwrap_or_else(|| {
            std::env::var(THING_NAME_ENV).unwrap_or_else(|_| DEFAULT_THING_NAME.to_string())
        });

        let source = config::source::build(&parsed.config)?;
        let raw = source.load().await?;
        config::validation::validate(&raw)?;
        let cfg = Config::from_value(self.component_name.clone(), thing_name, raw)?;

        logging::init(&cfg);

        tracing::info!(
            component = %self.component_name,
            thing = %cfg.thing_name,
            config_source = source.source_name(),
            "GGCommons initialized"
        );

        let messaging = init_messaging(&parsed.mode).await?;

        Ok(GgCommons {
            component_name: self.component_name,
            args: parsed,
            config: Arc::new(ArcSwap::from_pointee(cfg)),
            messaging,
        })
    }
}

/// Initialize the messaging service for the selected runtime mode.
///
/// # Purpose
/// In STANDALONE mode, load the messaging config and connect the dual-broker MQTT
/// provider; in GREENGRASS mode, messaging is deferred to Phase 2 (returns `None`).
///
/// # Semantics & Syntax
/// - **Signature**: `async fn init_messaging(mode: &RuntimeMode) -> Result<Option<Arc<dyn MessagingService>>>`
///
/// # Errors
/// | Error Variant | Condition | Recovery |
/// |---------------|-----------|----------|
/// | `GgError::Io` / `GgError::Json` | Messaging config file missing or malformed | Check the `-m STANDALONE <path>` file |
/// | `GgError::Messaging` | Broker connection failed, or `standalone` feature disabled | Verify the broker; enable the feature |
async fn init_messaging(
    mode: &cli::RuntimeMode,
) -> Result<Option<Arc<dyn messaging::MessagingService>>> {
    match mode {
        cli::RuntimeMode::Standalone {
            messaging_config_path,
        } => {
            #[cfg(feature = "standalone")]
            {
                use crate::messaging::config::MessagingConfig;
                use crate::messaging::provider::mqtt::MqttProvider;
                use crate::messaging::service::DefaultMessagingService;

                let mc = MessagingConfig::load(messaging_config_path).await?;
                let provider = Arc::new(MqttProvider::connect(&mc).await?);
                let service: Arc<dyn messaging::MessagingService> =
                    Arc::new(DefaultMessagingService::new(provider));
                Ok(Some(service))
            }
            #[cfg(not(feature = "standalone"))]
            {
                let _ = messaging_config_path;
                Err(GgError::Messaging(
                    "STANDALONE messaging requires the 'standalone' cargo feature".to_string(),
                ))
            }
        }
        cli::RuntimeMode::Greengrass => Ok(None), // Phase 2: Greengrass IPC messaging
    }
}

/// Common imports for component authors.
pub mod prelude {
    pub use crate::cli::{ConfigSourceSpec, ParsedArgs, RuntimeMode};
    pub use crate::config::model::Config;
    pub use crate::messaging::{Destination, Qos};
    pub use crate::{GgCommons, GgCommonsBuilder, GgError, Result};
}
