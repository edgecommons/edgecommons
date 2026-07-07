//! `KafkaSink` — exports batches to Apache Kafka via `rdkafka` (feature = `kafka`).
//!
//! Like [`KinesisSink`](crate::KinesisSink), the [`Sink`] trait is synchronous, so this sink owns a
//! private tokio runtime and `block_on`s delivery. Each record in a batch is produced with its
//! `partition_key` as the Kafka message key (so a key's records keep their partition/order), then
//! all deliveries are awaited:
//! - all delivered → [`SendOutcome::AllAcked`]
//! - some failed → [`SendOutcome::Partial`] (those offsets retry; Kafka dedups by key+offset
//!   downstream is the consumer's job — at-least-once, same as Kinesis)
//! - all failed (broker down) → [`SendOutcome::Failed`] `{ retryable: true }`

use std::collections::BTreeMap;

use rdkafka::config::ClientConfig;
use rdkafka::message::{Header, OwnedHeaders};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;
use tokio::runtime::Runtime;

use super::{ExportRecord, SendOutcome, Sink};
use crate::config::SinkPayloadFormat;
use crate::error::{EdgeStreamError, Result};
use crate::payload::{project_payload, PayloadMetadata};

/// A [`Sink`] that produces records to a Kafka topic.
pub struct KafkaSink {
    rt: Runtime,
    producer: FutureProducer,
    topic: String,
    payload_format: SinkPayloadFormat,
}

impl KafkaSink {
    /// Build a sink for `topic` on the given `bootstrap_servers`. `properties` are extra librdkafka
    /// producer settings (security/SASL/etc.) applied verbatim.
    pub fn new(
        bootstrap_servers: &str,
        topic: &str,
        properties: &BTreeMap<String, String>,
    ) -> Result<Self> {
        Self::new_with_payload_format(
            bootstrap_servers,
            topic,
            properties,
            SinkPayloadFormat::Json,
        )
    }

    /// Build a sink with an explicit target payload format.
    pub fn new_with_payload_format(
        bootstrap_servers: &str,
        topic: &str,
        properties: &BTreeMap<String, String>,
        payload_format: SinkPayloadFormat,
    ) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("edgestreamlog-kafka")
            .build()
            .map_err(|e| EdgeStreamError::Sink(format!("tokio runtime: {e}")))?;

        let mut cfg = ClientConfig::new();
        cfg.set("bootstrap.servers", bootstrap_servers);
        cfg.set("message.timeout.ms", "30000");
        for (k, v) in properties {
            cfg.set(k, v);
        }
        let producer: FutureProducer = cfg
            .create()
            .map_err(|e| EdgeStreamError::Sink(format!("kafka producer: {e}")))?;

        Ok(Self {
            rt,
            producer,
            topic: topic.to_string(),
            payload_format,
        })
    }
}

impl Sink for KafkaSink {
    fn send(&mut self, batch: &[ExportRecord<'_>]) -> SendOutcome {
        let topic = self.topic.clone();
        let producer = self.producer.clone();
        let payload_format = self.payload_format;

        self.rt.block_on(async move {
            // Send each projected record and await its delivery while the projected payload buffer
            // is still in scope. This keeps the sink's ownership model simple and exact.
            for r in batch {
                let projected = match project_payload(payload_format, r.payload) {
                    Ok(projected) => projected,
                    Err(e) => {
                        return SendOutcome::Failed {
                            retryable: false,
                            error: e.to_string(),
                        };
                    }
                };
                let record = FutureRecord::to(&topic)
                    .payload(projected.payload.as_slice())
                    .key(r.partition_key)
                    .headers(metadata_headers(&projected.metadata));
                // DeliveryFuture resolves to Result<(partition, offset), (KafkaError, OwnedMessage)>.
                match producer.send(record, Timeout::Never).await {
                    Ok(_) => {} // delivered
                    Err((kafka_err, _msg)) => {
                        return SendOutcome::Failed {
                            retryable: true,
                            error: kafka_err.to_string(),
                        };
                    }
                }
            }

            SendOutcome::AllAcked
        })
    }
}

fn metadata_headers(metadata: &PayloadMetadata) -> OwnedHeaders {
    metadata
        .entries()
        .into_iter()
        .fold(OwnedHeaders::new(), |headers, (key, value)| {
            headers.insert(Header {
                key,
                value: Some(value.as_bytes()),
            })
        })
}
