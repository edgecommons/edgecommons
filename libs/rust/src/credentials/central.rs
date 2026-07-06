//! # Central vault sources
//!
//! **One-liner purpose**: The upstream source-of-truth a vault is seeded and refreshed from — a
//! pluggable [`CentralVaultSource`] with a built-in AWS Secrets Manager implementation (feature
//! `credentials-aws`).
//!
//! ## Semantics & Architecture
//! - Pull-only in v1: [`CentralVaultSource::fetch`] returns the current value + an upstream
//!   version id for change detection. The [`super::sync::SyncEngine`] writes pulled values into the
//!   local vault.
//! - The AWS source owns a private tokio runtime and `block_on`s each call; the client is loaded on
//!   a dedicated thread so construction is safe inside the library's async `build()`.

use std::collections::BTreeMap;

use crate::Result;

/// A secret value fetched from the central source.
pub struct CentralSecret {
    pub bytes: Vec<u8>,
    /// Upstream version id (e.g. Secrets Manager `VersionId`) for change detection.
    pub central_version_id: String,
    pub labels: BTreeMap<String, String>,
}

/// The upstream source a vault syncs from. Implementations must be `Send + Sync`.
pub trait CentralVaultSource: Send + Sync {
    /// Fetch the current value of `name`, or `None` if it does not exist upstream.
    fn fetch(&self, name: &str) -> Result<Option<CentralSecret>>;
}

#[cfg(feature = "credentials-aws")]
pub use aws::AwsSecretsManagerSource;

#[cfg(feature = "credentials-aws")]
mod aws {
    use super::{CentralSecret, CentralVaultSource};
    use crate::Result;
    use crate::error::EdgeCommonsError;
    use aws_sdk_secretsmanager::Client;
    use aws_sdk_secretsmanager::error::DisplayErrorContext;
    use std::collections::BTreeMap;
    use tokio::runtime::Runtime;

    /// Central source backed by AWS Secrets Manager. Auth = AWS default chain (TES on Greengrass);
    /// `endpoint_url` overrides for an emulator (floci/LocalStack) or a VPC endpoint.
    pub struct AwsSecretsManagerSource {
        rt: Runtime,
        client: Client,
    }

    impl AwsSecretsManagerSource {
        pub fn new(region: Option<String>, endpoint_url: Option<String>) -> Result<Self> {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("edgecommons-secretsmanager")
                .build()
                .map_err(|e| EdgeCommonsError::Credentials(format!("tokio runtime: {e}")))?;

            // Load the client on a dedicated OS thread: `block_on` on a thread already driving a
            // runtime (the library's async build()) would panic. See KinesisSink for the same fix.
            let client = std::thread::scope(|scope| {
                scope
                    .spawn(|| {
                        rt.block_on(async {
                            let mut loader =
                                aws_config::defaults(aws_config::BehaviorVersion::latest());
                            if let Some(r) = region {
                                loader =
                                    loader.region(aws_sdk_secretsmanager::config::Region::new(r));
                            }
                            if let Some(url) = endpoint_url {
                                loader = loader.endpoint_url(url);
                            }
                            let conf = loader.load().await;
                            Client::new(&conf)
                        })
                    })
                    .join()
                    .map_err(|_| {
                        EdgeCommonsError::Credentials(
                            "secretsmanager client init thread panicked".into(),
                        )
                    })
            })?;

            Ok(Self { rt, client })
        }
    }

    impl CentralVaultSource for AwsSecretsManagerSource {
        fn fetch(&self, name: &str) -> Result<Option<CentralSecret>> {
            let resp = self
                .rt
                .block_on(self.client.get_secret_value().secret_id(name).send());
            match resp {
                Ok(o) => {
                    let central_version_id = o.version_id().unwrap_or_default().to_string();
                    let bytes = if let Some(s) = o.secret_string() {
                        s.as_bytes().to_vec()
                    } else if let Some(b) = o.secret_binary() {
                        b.as_ref().to_vec()
                    } else {
                        return Ok(None);
                    };
                    Ok(Some(CentralSecret {
                        bytes,
                        central_version_id,
                        labels: BTreeMap::new(),
                    }))
                }
                Err(e) => {
                    if e.as_service_error()
                        .map(|s| s.is_resource_not_found_exception())
                        .unwrap_or(false)
                    {
                        Ok(None)
                    } else {
                        Err(EdgeCommonsError::Credentials(format!(
                            "get secret '{name}': {}",
                            DisplayErrorContext(&e)
                        )))
                    }
                }
            }
        }
    }
}
