//! `KinesisSink` — exports batches via Kinesis `PutRecords` (feature = `kinesis`).
//!
//! The [`Sink`] trait is synchronous, so this sink owns a private multi-thread tokio runtime
//! and `block_on`s each `PutRecords` call. The engine thread stays tokio-free; one runtime per
//! sink is fine (Phase 1 is one export thread per stream).
//!
//! Mapping to [`SendOutcome`]:
//! - `failed_record_count == 0` → [`SendOutcome::AllAcked`].
//! - per-entry `error_code` set → those offsets become [`SendOutcome::Partial`] (retried; Kinesis
//!   does not double-store the acked entries, so this adds no duplicates).
//! - transport / throttling / 5xx errors → [`SendOutcome::Failed`] `{ retryable: true }`.
//!
//! `PutRecords` caps (500 records / 5 MiB per call) must be enforced by `BatchConfig`; this sink
//! sends whatever batch it is handed.

use aws_sdk_kinesis::error::DisplayErrorContext;
use aws_sdk_kinesis::primitives::Blob;
use aws_sdk_kinesis::types::PutRecordsRequestEntry;
use aws_sdk_kinesis::Client;
use tokio::runtime::Runtime;

use super::{ExportRecord, SendOutcome, Sink};
use crate::error::{GgStreamError, Result};

/// A [`Sink`] that writes records to an Amazon Kinesis Data Stream.
pub struct KinesisSink {
    rt: Runtime,
    client: Client,
    stream_name: String,
}

impl KinesisSink {
    /// Build a sink using the AWS default credential/region provider chain (env, profile, IMDS,
    /// and the Greengrass TES container-credentials endpoint all work with no extra code).
    ///
    /// `region` overrides the chain's region when `Some`. `endpoint_url` overrides the Kinesis
    /// endpoint (LocalStack, a VPC endpoint, or tests) when `Some`.
    pub fn new(
        stream_name: impl Into<String>,
        region: Option<String>,
        endpoint_url: Option<String>,
    ) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("ggstreamlog-kinesis")
            .build()
            .map_err(|e| GgStreamError::Sink(format!("tokio runtime: {e}")))?;

        let client = rt.block_on(async move {
            let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
            if let Some(r) = region {
                loader = loader.region(aws_sdk_kinesis::config::Region::new(r));
            }
            if let Some(url) = endpoint_url {
                loader = loader.endpoint_url(url);
            }
            let conf = loader.load().await;
            Client::new(&conf)
        });

        Ok(Self { rt, client, stream_name: stream_name.into() })
    }
}

impl Sink for KinesisSink {
    fn send(&mut self, batch: &[ExportRecord<'_>]) -> SendOutcome {
        let entries: std::result::Result<Vec<PutRecordsRequestEntry>, _> = batch
            .iter()
            .map(|r| {
                PutRecordsRequestEntry::builder()
                    .data(Blob::new(r.payload.to_vec()))
                    // Kinesis partition keys are UTF-8 strings; recover lossily from the raw bytes.
                    .partition_key(String::from_utf8_lossy(r.partition_key).into_owned())
                    .build()
            })
            .collect();
        let entries = match entries {
            Ok(e) => e,
            Err(e) => {
                // A malformed entry can never succeed → non-retryable.
                return SendOutcome::Failed { retryable: false, error: format!("build entry: {e}") };
            }
        };

        let resp = self.rt.block_on(
            self.client
                .put_records()
                .stream_name(&self.stream_name)
                .set_records(Some(entries))
                .send(),
        );

        match resp {
            Ok(out) => {
                if out.failed_record_count().unwrap_or(0) == 0 {
                    return SendOutcome::AllAcked;
                }
                // Result entries are positionally aligned with the request records.
                let mut failed_offsets = Vec::new();
                let mut last_err: Option<String> = None;
                for (entry, rec) in out.records().iter().zip(batch.iter()) {
                    if let Some(code) = entry.error_code() {
                        failed_offsets.push(rec.offset);
                        last_err = Some(match entry.error_message() {
                            Some(m) => format!("{code}: {m}"),
                            None => code.to_string(),
                        });
                    }
                }
                if failed_offsets.is_empty() {
                    // Count was non-zero but nothing mapped — retry the whole batch defensively.
                    SendOutcome::Failed {
                        retryable: true,
                        error: "PutRecords reported failures with no per-entry error code".into(),
                    }
                } else {
                    let _ = last_err; // captured for parity; engine logs from Failed only
                    SendOutcome::Partial { failed_offsets }
                }
            }
            Err(e) => SendOutcome::Failed {
                // Treat dispatch/throttle/5xx as retryable; the engine's max_retries governs giving up.
                retryable: true,
                error: format!("{}", DisplayErrorContext(&e)),
            },
        }
    }
}
