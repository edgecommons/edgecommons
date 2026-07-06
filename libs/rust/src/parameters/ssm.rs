//! # AWS SSM Parameter Store source (feature = `parameters-aws`)
//!
//! Reads parameters from AWS SSM via `GetParameter` / `GetParametersByPath` (with decryption, so
//! `SecureString`s resolve and are flagged `secure`). The AWS client is built on a dedicated
//! thread/runtime (like the Secrets Manager source) so construction is safe inside the library's
//! async `build()`. Uses the default credential chain — TES on Greengrass, ambient creds in
//! STANDALONE.

use aws_sdk_ssm::Client;
use aws_sdk_ssm::error::DisplayErrorContext;
use aws_sdk_ssm::types::{Parameter, ParameterType};
use tokio::runtime::Runtime;

use super::source::{ParamValue, ParameterSource};
use crate::Result;
use crate::error::EdgeCommonsError;

/// AWS SSM Parameter Store [`ParameterSource`].
pub struct AwsSsmSource {
    rt: Runtime,
    client: Client,
    with_decryption: bool,
}

impl AwsSsmSource {
    /// Build the SSM client (dedicated thread; `endpoint_url` overrides for floci/LocalStack/VPC).
    pub fn new(
        region: Option<String>,
        endpoint_url: Option<String>,
        with_decryption: bool,
    ) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("edgecommons-ssm")
            .build()
            .map_err(|e| EdgeCommonsError::Parameters(format!("tokio runtime: {e}")))?;
        let client = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    rt.block_on(async {
                        let mut loader =
                            aws_config::defaults(aws_config::BehaviorVersion::latest());
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
                .map_err(|_| EdgeCommonsError::Parameters("ssm client init thread panicked".into()))
        })?;
        Ok(Self {
            rt,
            client,
            with_decryption,
        })
    }

    fn to_value(p: &Parameter) -> Option<ParamValue> {
        let value = p.value()?.to_string();
        let secure = p.r#type() == Some(&ParameterType::SecureString);
        Some(ParamValue {
            value: value.into_bytes(),
            secure,
            version: Some(p.version().to_string()),
        })
    }
}

impl ParameterSource for AwsSsmSource {
    fn fetch(&self, name: &str) -> Result<Option<ParamValue>> {
        let resp = self.rt.block_on(
            self.client
                .get_parameter()
                .name(name)
                .with_decryption(self.with_decryption)
                .send(),
        );
        match resp {
            Ok(r) => Ok(r.parameter().and_then(Self::to_value)),
            Err(e) => {
                let svc = e.into_service_error();
                if svc.is_parameter_not_found() {
                    Ok(None)
                } else {
                    Err(EdgeCommonsError::Parameters(format!(
                        "ssm get_parameter: {}",
                        DisplayErrorContext(&svc)
                    )))
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
            let resp = self.rt.block_on(req.send()).map_err(|e| {
                EdgeCommonsError::Parameters(format!(
                    "ssm get_parameters_by_path: {}",
                    DisplayErrorContext(&e)
                ))
            })?;
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

#[cfg(test)]
mod floci_it {
    //! End-to-end test of the real SSM client path against a local AWS emulator (floci or
    //! LocalStack — both speak SSM on `:4566`). **Ignored by default** (needs the emulator).
    //!
    //! ```sh
    //! curl -s localhost:4566/_floci/health    # confirm "ssm":"running"
    //! cargo test -p edgecommons --features parameters-aws ssm::floci_it -- --ignored --nocapture
    //! ```
    //! Override the endpoint with `EDGECOMMONS_SSM_ENDPOINT` (default `http://localhost:4566`).
    use super::*;
    use std::collections::HashMap;

    fn endpoint() -> String {
        std::env::var("EDGECOMMONS_SSM_ENDPOINT").unwrap_or_else(|_| "http://localhost:4566".into())
    }

    #[test]
    #[ignore = "requires a local AWS emulator (floci/LocalStack) with SSM on :4566"]
    fn ssm_source_reads_string_secure_and_by_path() {
        // Static creds + region via env so both the admin (seed) client and the AwsSsmSource
        // under test resolve the same way they would against real AWS.
        unsafe {
            std::env::set_var("AWS_ACCESS_KEY_ID", "test");
            std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
            std::env::set_var("AWS_DEFAULT_REGION", "us-east-1");
        }
        let prefix = format!("/edgecommons-it-{}", std::process::id());

        // --- seed floci via the SDK admin client (put String + SecureString + a 2-key tree) ---
        let rt = Runtime::new().unwrap();
        let admin = rt.block_on(async {
            let conf = aws_config::defaults(aws_config::BehaviorVersion::latest())
                .region(aws_sdk_ssm::config::Region::new("us-east-1"))
                .endpoint_url(endpoint())
                .load()
                .await;
            Client::new(&conf)
        });
        let put = |name: String, value: &'static str, ty: ParameterType| {
            let admin = admin.clone();
            rt.block_on(async move {
                admin
                    .put_parameter()
                    .name(name)
                    .value(value)
                    .r#type(ty)
                    .overwrite(true)
                    .send()
                    .await
                    .expect("seed put_parameter");
            });
        };
        put(
            format!("{prefix}/plain"),
            "us-east-1",
            ParameterType::String,
        );
        put(
            format!("{prefix}/secure"),
            "p@ss",
            ParameterType::SecureString,
        );
        put(format!("{prefix}/tree/a"), "1", ParameterType::String);
        put(format!("{prefix}/tree/b"), "2", ParameterType::String);

        // --- read back via the source under test (with_decryption = true) ---
        let src = AwsSsmSource::new(Some("us-east-1".into()), Some(endpoint()), true).unwrap();

        let plain = src
            .fetch(&format!("{prefix}/plain"))
            .unwrap()
            .expect("plain present");
        assert_eq!(String::from_utf8(plain.value).unwrap(), "us-east-1");
        assert!(!plain.secure, "String must not be flagged secure");
        assert!(plain.version.is_some(), "version should be populated");

        let secure = src
            .fetch(&format!("{prefix}/secure"))
            .unwrap()
            .expect("secure present");
        assert_eq!(
            String::from_utf8(secure.value).unwrap(),
            "p@ss",
            "SecureString must decrypt"
        );
        assert!(secure.secure, "SecureString must be flagged secure");

        assert!(
            src.fetch(&format!("{prefix}/missing")).unwrap().is_none(),
            "missing -> None"
        );

        let tree: HashMap<String, String> = src
            .fetch_by_path(&format!("{prefix}/tree"), true)
            .unwrap()
            .into_iter()
            .map(|(k, v)| (k, String::from_utf8(v.value).unwrap()))
            .collect();
        assert_eq!(
            tree.get(&format!("{prefix}/tree/a")).map(String::as_str),
            Some("1")
        );
        assert_eq!(
            tree.get(&format!("{prefix}/tree/b")).map(String::as_str),
            Some("2")
        );

        // cleanup (best-effort)
        for suffix in ["/plain", "/secure", "/tree/a", "/tree/b"] {
            let name = format!("{prefix}{suffix}");
            let admin = admin.clone();
            let _ = rt.block_on(async move { admin.delete_parameter().name(name).send().await });
        }
    }
}
