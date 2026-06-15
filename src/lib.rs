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

        Ok(GgCommons {
            component_name: self.component_name,
            args: parsed,
            config: Arc::new(ArcSwap::from_pointee(cfg)),
        })
    }
}

/// Common imports for component authors.
pub mod prelude {
    pub use crate::cli::{ConfigSourceSpec, ParsedArgs, RuntimeMode};
    pub use crate::config::model::Config;
    pub use crate::messaging::{Destination, Qos};
    pub use crate::{GgCommons, GgCommonsBuilder, GgError, Result};
}
