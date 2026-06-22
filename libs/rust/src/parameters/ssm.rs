//! # AWS SSM Parameter Store source (feature = `parameters-aws`)
//!
//! Reads parameters from AWS SSM via `GetParameter` / `GetParametersByPath` (with decryption, so
//! `SecureString`s resolve and are flagged `secure`). The AWS client is built on a dedicated
//! thread/runtime (like the Secrets Manager source) so construction is safe inside the library's
//! async `build()`. Uses the default credential chain — TES on Greengrass, ambient creds in
//! STANDALONE.

use aws_sdk_ssm::error::DisplayErrorContext;
use aws_sdk_ssm::types::{Parameter, ParameterType};
use aws_sdk_ssm::Client;
use tokio::runtime::Runtime;

use super::source::{ParamValue, ParameterSource};
use crate::error::GgError;
use crate::Result;

/// AWS SSM Parameter Store [`ParameterSource`].
pub struct AwsSsmSource {
    rt: Runtime,
    client: Client,
    with_decryption: bool,
}

impl AwsSsmSource {
    /// Build the SSM client (dedicated thread; `endpoint_url` overrides for floci/LocalStack/VPC).
    pub fn new(region: Option<String>, endpoint_url: Option<String>, with_decryption: bool) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("ggcommons-ssm")
            .build()
            .map_err(|e| GgError::Parameters(format!("tokio runtime: {e}")))?;
        let client = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    rt.block_on(async {
                        let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
                        if let Some(r) = region {
                            loader = loader.region(aws_sdk_ssm::config::Region::new(r));
                        }
                        if let Some(url) = endpoint_url {
                            loader = loader.endpoint_url(url);
                        }
                        Client::new(&loader.load().await)
                    })
                })
                .join()
                .map_err(|_| GgError::Parameters("ssm client init thread panicked".into()))
        })?;
        Ok(Self { rt, client, with_decryption })
    }

    fn to_value(p: &Parameter) -> Option<ParamValue> {
        let value = p.value()?.to_string();
        let secure = p.r#type() == Some(&ParameterType::SecureString);
        Some(ParamValue { value: value.into_bytes(), secure, version: Some(p.version().to_string()) })
    }
}

impl ParameterSource for AwsSsmSource {
    fn fetch(&self, name: &str) -> Result<Option<ParamValue>> {
        let resp = self.rt.block_on(
            self.client.get_parameter().name(name).with_decryption(self.with_decryption).send(),
        );
        match resp {
            Ok(r) => Ok(r.parameter().and_then(Self::to_value)),
            Err(e) => {
                let svc = e.into_service_error();
                if svc.is_parameter_not_found() {
                    Ok(None)
                } else {
                    Err(GgError::Parameters(format!("ssm get_parameter: {}", DisplayErrorContext(&svc))))
                }
            }
        }
    }

    fn fetch_by_path(&self, path: &str, recursive: bool) -> Result<Vec<(String, ParamValue)>> {
        let mut out = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let mut req = self
                .client
                .get_parameters_by_path()
                .path(path)
                .recursive(recursive)
                .with_decryption(self.with_decryption);
            if let Some(t) = &next {
                req = req.next_token(t);
            }
            let resp = self
                .rt
                .block_on(req.send())
                .map_err(|e| GgError::Parameters(format!("ssm get_parameters_by_path: {}", DisplayErrorContext(&e))))?;
            for p in resp.parameters() {
                if let (Some(name), Some(v)) = (p.name(), Self::to_value(p)) {
                    out.push((name.to_string(), v));
                }
            }
            next = resp.next_token().map(|s| s.to_string());
            if next.is_none() {
                break;
            }
        }
        Ok(out)
    }

    fn source_id(&self) -> &str {
        "awsSsm"
    }
}
