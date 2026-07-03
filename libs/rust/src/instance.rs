//! # GgInstance — the per-instance seam
//!
//! **One-liner purpose**: The instance-scoped handle (UNS-CANONICAL-DESIGN §3,
//! D-U3) whose only job is to pre-bind the instance token into (a) the [`Uns`]
//! topic builder and (b) the [`MessageBuilder`].
//!
//! The messaging service stays instance-agnostic — `publish(topic, msg)` already
//! receives both the topic (minted by this handle's instance-bound [`Uns`]) and the
//! envelope (stamped by its instance-bound builder). Component-level messages
//! (everything not built through a handle) default to instance `"main"`.
//!
//! Obtain handles from [`crate::GgCommons::instance`] (token validated against the
//! §2.2 rule). The id is deliberately NOT verified against the configured
//! `component.instances[]` — instances may be created dynamically; an unknown id is
//! only logged at DEBUG as a diagnostic aid.
//!
//! ## Usage Example
//! ```no_run
//! # async fn demo(gg: &ggcommons::GgCommons) -> ggcommons::Result<()> {
//! use ggcommons::uns::UnsClass;
//! let kep1 = gg.instance("kep1")?;
//! let topic = kep1.uns().topic_with_channel(UnsClass::Data, "temp")?;
//! let msg = kep1.message("data", "1.0").payload(serde_json::json!({ "v": 1 })).build();
//! gg.messaging()?.publish(&topic, &msg).await?;
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;

use crate::config::model::Config;
use crate::error::Result;
use crate::messaging::message::MessageBuilder;
use crate::uns::Uns;

/// The per-instance seam (UNS-CANONICAL-DESIGN §3, D-U3): an instance-scoped
/// handle over a configuration snapshot. See the [module docs](self).
pub struct GgInstance {
    id: String,
    config: Arc<Config>,
    uns: Uns,
}

impl GgInstance {
    /// Crate-private: created by [`crate::GgCommons::instance`], which validates
    /// the token (§2.2 token rule) first.
    pub(crate) fn new(id: String, config: Arc<Config>) -> Result<GgInstance> {
        let identity = config.identity().with_instance(id.clone())?;
        // The RAW includeRoot flag, like gg.uns(): Uns applies it per-target only
        // for multi-level hierarchies (D-U25).
        let uns = Uns::new(identity, config.topic_include_root());
        Ok(GgInstance { id, config, uns })
    }

    /// Returns this handle's instance token.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the topic builder bound to this instance (topics minted with this
    /// instance token).
    pub fn uns(&self) -> &Uns {
        &self.uns
    }

    /// Starts a message pre-bound to this instance — equivalent to
    /// `MessageBuilder::new(name, version).from_config(&config).instance(id())`, so
    /// `build()` stamps the component identity with this handle's instance token.
    pub fn message(&self, name: impl Into<String>, version: impl Into<String>) -> MessageBuilder {
        MessageBuilder::new(name, version)
            .from_config(&self.config)
            .instance(self.id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uns::UnsClass;
    use serde_json::json;

    fn config() -> Arc<Config> {
        Arc::new(Config::from_value("com.example.MyComp", "gw-01", json!({})).unwrap())
    }

    #[test]
    fn handle_binds_the_instance_into_uns_and_messages() {
        let handle = GgInstance::new("kep1".to_string(), config()).unwrap();
        assert_eq!(handle.id(), "kep1");
        assert_eq!(
            handle.uns().topic_with_channel(UnsClass::Data, "temp").unwrap(),
            "ecv1/gw-01/MyComp/kep1/data/temp"
        );
        let msg = handle.message("data", "1.0").payload(json!({ "v": 1 })).build();
        assert_eq!(msg.identity.unwrap().instance(), "kep1");
        assert_eq!(msg.header.name, "data");
    }
}
