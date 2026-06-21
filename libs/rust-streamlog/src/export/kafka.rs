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
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;
use tokio::runtime::Runtime;

use super::{ExportRecord, SendOutcome, Sink};
use crate::error::{GgStreamError, Result};

/// A [`Sink`] that produces records to a Kafka topic.
pub struct KafkaSink {
    rt: Runtime,
    producer: FutureProducer,
    topic: String,
}

impl KafkaSink {
    /// Build a sink for `topic` on the given `bootstrap_servers`. `properties` are extra librdkafka
    /// producer settings (security/SASL/etc.) applied verbatim.
    pub fn new(
        bootstrap_servers: &str,
        topic: &str,
        properties: &BTreeMap<String, String>,
    ) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("ggstreamlog-kafka")
            .build()
            .map_err(|e| GgStreamError::Sink(format!("tokio runtime: {e}")))?;

        let mut cfg = ClientConfig::new();
        cfg.set("bootstrap.servers", bootstrap_servers);
        cfg.set("message.timeout.ms", "30000");
        for (k, v) in properties {
            cfg.set(k, v);
        }
        let producer: FutureProducer =
            cfg.create().map_err(|e| GgStreamError::Sink(format!("kafka producer: {e}")))?;

        Ok(Self { rt, producer, topic: topic.to_string() })
    }
}

impl Sink for KafkaSink {
    fn send(&mut self, batch: &[ExportRecord<'_>]) -> SendOutcome {
        let topic = &self.topic;
        let producer = &self.producer;

        self.rt.block_on(async move {
            // Queue all sends first (concurrent in-flight), then await each delivery.
            let mut pending = Vec::with_capacity(batch.len());
            for r in batch {
                let record = FutureRecord::to(topic).payload(r.payload).key(r.partition_key);
                pending.push((r.offset, producer.send(record, Timeout::Never)));
            }

            let mut failed_offsets = Vec::new();
            let mut last_err = None;
            for (offset, fut) in pending {
                // DeliveryFuture resolves to Result<(partition, offset), (KafkaError, OwnedMessage)>.
                match fut.await {
                    Ok(_) => {} // delivered
                    Err((kafka_err, _msg)) => {
                        failed_offsets.push(offset);
                        last_err = Some(kafka_err.to_string());
                    }
                }
            }

            if failed_offsets.is_empty() {
                SendOutcome::AllAcked
            } else if failed_offsets.len() == batch.len() {
                SendOutcome::Failed {
                    retryable: true,
                    error: last_err.unwrap_or_else(|| "all records failed to deliver".into()),
                }
            } else {
                SendOutcome::Partial { failed_offsets }
            }
        })
    }
}
