/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.metrics.targets;

import com.breissinger.ggcommons.streaming.SinkOutcome;
import com.breissinger.ggcommons.streaming.SinkRecord;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.services.cloudwatch.model.MetricDatum;

import java.time.Duration;
import java.time.Instant;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.atomic.AtomicLong;

/**
 * The durable-CloudWatch drain: the host-callback sink logic for a {@code callback}-type
 * ggstreamlog stream of {@link CloudWatchRecord} payloads. Reused for every export batch the engine
 * hands to the buffer's sink. Pure (no native/FFI) so it is fully unit-testable with a
 * {@link CloudWatchSender} stub.
 *
 * <p>Per batch it: (1) deserializes each record; (2) drops datums whose timestamp falls outside
 * CloudWatch's accept window (~2 weeks past to ~2 hours future) and counts them; (3) drops malformed
 * records; (4) groups the survivors by namespace; (5) chunks each namespace to ≤1000 datums /
 * ≤~1 MB; (6) sends each chunk via {@link CloudWatchSender#send}; (7) maps the result onto a
 * {@link SinkOutcome} (all sent → {@code ALL_ACKED}; a failed chunk's offsets → retried via
 * {@code PARTIAL}, or whole-batch {@code FAILED_RETRYABLE} if every datum failed). Dropped
 * (stale/malformed) records are treated as acked — retry cannot fix an aged-out timestamp or garbage.
 */
public final class CloudWatchDrain {

    private static final Logger LOGGER = LogManager.getLogger(CloudWatchDrain.class);

    /** PutMetricData accepts at most 1000 datums per request. */
    static final int MAX_DATUMS_PER_REQUEST = 1000;
    /** PutMetricData accepts at most ~1 MB per request; keep a safety margin under the hard limit. */
    static final int MAX_BYTES_PER_REQUEST = 900_000;
    /**
     * Floor for a datum's estimated byte cost. Each datum's cost is taken from its serialized record
     * length (a tight upper bound on the PutMetricData payload it produces); this floor guards
     * against a degenerate tiny estimate. For typical datums the 1000-datum count limit governs and
     * the ~1 MB byte limit only forces an early chunk boundary for unusually large datums.
     */
    static final int MIN_DATUM_BYTES = 64;

    /** CloudWatch accepts timestamps up to ~2 weeks in the past. */
    static final Duration MAX_PAST = Duration.ofDays(14);
    /** CloudWatch accepts timestamps up to ~2 hours in the future. */
    static final Duration MAX_FUTURE = Duration.ofHours(2);

    private final CloudWatchSender sender;
    private final AtomicLong droppedStale = new AtomicLong();
    private final AtomicLong droppedMalformed = new AtomicLong();

    public CloudWatchDrain(CloudWatchSender sender) {
        this.sender = sender;
    }

    /** Total datums dropped for being outside the CloudWatch accept window (lifetime). */
    public long droppedStale() {
        return droppedStale.get();
    }

    /** Total records dropped for being unparseable (lifetime). */
    public long droppedMalformed() {
        return droppedMalformed.get();
    }

    /** Drain one batch using the wall clock; see {@link #drain(List, Instant)}. */
    public SinkOutcome drain(List<SinkRecord> batch) {
        return drain(batch, Instant.now());
    }

    /**
     * Drain one batch, judging staleness relative to {@code now} (injectable for tests).
     *
     * @param batch the records the export engine handed the sink (never empty in practice)
     * @param now   reference instant for the accept-window check
     * @return the outcome to report to the engine (drives commit/retry)
     */
    public SinkOutcome drain(List<SinkRecord> batch, Instant now) {
        Instant oldest = now.minus(MAX_PAST);
        Instant newest = now.plus(MAX_FUTURE);

        // Group live datums by namespace, preserving each datum's source offset so a failed chunk
        // can be reported back precisely. Insertion-ordered for deterministic sends.
        Map<String, List<Entry>> byNamespace = new LinkedHashMap<>();
        for (SinkRecord rec : batch) {
            CloudWatchRecord.Parsed parsed;
            try {
                parsed = CloudWatchRecord.deserialize(rec.payload());
            } catch (RuntimeException e) {
                droppedMalformed.incrementAndGet();
                LOGGER.warn("Dropping malformed CloudWatch buffer record at offset {}: {}",
                        rec.offset(), e.getMessage());
                continue;
            }
            Instant ts = parsed.timestamp();
            if (ts.isBefore(oldest) || ts.isAfter(newest)) {
                droppedStale.incrementAndGet();
                LOGGER.debug("Dropping stale CloudWatch datum (ts={} outside [{}, {}])",
                        ts, oldest, newest);
                continue;
            }
            // Estimate the datum's PutMetricData payload cost from its serialized record length
            // (a tight upper bound: the record JSON wraps the same name/value/dims).
            int estBytes = Math.max(rec.payload().length, MIN_DATUM_BYTES);
            byNamespace.computeIfAbsent(parsed.namespace(), k -> new ArrayList<>())
                    .add(new Entry(rec.offset(), parsed.datum(), estBytes));
        }

        List<Long> failedOffsets = new ArrayList<>();
        int sentCount = 0;

        for (Map.Entry<String, List<Entry>> nsEntry : byNamespace.entrySet()) {
            String namespace = nsEntry.getKey();
            List<Entry> entries = nsEntry.getValue();
            // Chunk by both the 1000-datum and ~1 MB limits.
            int i = 0;
            while (i < entries.size()) {
                List<MetricDatum> chunk = new ArrayList<>();
                List<Long> chunkOffsets = new ArrayList<>();
                int bytes = 0;
                while (i < entries.size()
                        && chunk.size() < MAX_DATUMS_PER_REQUEST
                        && (chunk.isEmpty() || bytes + entries.get(i).estBytes() <= MAX_BYTES_PER_REQUEST)) {
                    Entry e = entries.get(i);
                    chunk.add(e.datum());
                    chunkOffsets.add(e.offset());
                    bytes += e.estBytes();
                    i++;
                }
                try {
                    sender.send(namespace, chunk);
                    sentCount += chunk.size();
                    LOGGER.debug("Sent {} datums to CloudWatch namespace {}", chunk.size(), namespace);
                } catch (RuntimeException ex) {
                    // The chunk was not accepted — hold its records for retry (at-least-once).
                    failedOffsets.addAll(chunkOffsets);
                    LOGGER.warn("PutMetricData failed for namespace {} ({} datums); will retry: {}",
                            namespace, chunk.size(), ex.getMessage());
                }
            }
        }

        if (failedOffsets.isEmpty()) {
            // Everything sent or dropped (stale/malformed) → commit past the whole batch.
            return SinkOutcome.allAcked();
        }
        if (sentCount == 0) {
            // Nothing got through (typical disconnect) — hold the whole batch with a clean retry.
            return SinkOutcome.retryable();
        }
        // Some chunks got through, some did not — retry just the failed offsets.
        return SinkOutcome.partial(failedOffsets);
    }

    /** A datum paired with its source log offset (for precise partial-failure reporting) + byte cost. */
    private record Entry(long offset, MetricDatum datum, int estBytes) {
    }
}
