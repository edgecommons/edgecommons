/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.metrics.targets;

import com.mbreissi.edgecommons.streaming.SinkOutcome;
import com.mbreissi.edgecommons.streaming.SinkRecord;
import org.junit.jupiter.api.Test;
import software.amazon.awssdk.services.cloudwatch.model.MetricDatum;

import java.time.Instant;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.*;

/** Unit tests for the durable-CloudWatch drain logic (no native/FFI; stub sender). */
class CloudWatchDrainTest {

    private static final Instant NOW = Instant.ofEpochMilli(1_700_000_000_000L);

    /** A capturing sender; optionally fails sends to a given set of namespaces. */
    private static final class CaptureSender implements CloudWatchSender {
        final List<String> namespaces = new ArrayList<>();
        final List<Integer> chunkSizes = new ArrayList<>();
        final AtomicInteger calls = new AtomicInteger();
        Set<String> failNamespaces = new HashSet<>();

        @Override
        public void send(String namespace, List<MetricDatum> chunk) {
            calls.incrementAndGet();
            if (failNamespaces.contains(namespace)) {
                throw new RuntimeException("injected send failure for " + namespace);
            }
            namespaces.add(namespace);
            chunkSizes.add(chunk.size());
        }
    }

    private static SinkRecord record(long offset, String namespace, long tsMs) {
        MetricDatum datum = MetricDatum.builder()
                .metricName("m").value((double) offset).timestamp(Instant.ofEpochMilli(tsMs)).build();
        return new SinkRecord(offset, tsMs, namespace, CloudWatchRecord.serialize(namespace, datum));
    }

    @Test
    void allFreshSingleNamespaceAcksAll() {
        CaptureSender sender = new CaptureSender();
        CloudWatchDrain drain = new CloudWatchDrain(sender);
        List<SinkRecord> batch = List.of(
                record(0, "ns1", NOW.toEpochMilli()),
                record(1, "ns1", NOW.toEpochMilli()));

        SinkOutcome out = drain.drain(batch, NOW);

        assertEquals(SinkOutcome.Status.ALL_ACKED, out.status());
        assertEquals(1, sender.calls.get(), "one namespace -> one chunk");
        assertEquals(List.of(2), sender.chunkSizes);
        assertEquals(0, drain.droppedStale());
    }

    @Test
    void groupsByNamespace() {
        CaptureSender sender = new CaptureSender();
        CloudWatchDrain drain = new CloudWatchDrain(sender);
        List<SinkRecord> batch = List.of(
                record(0, "nsA", NOW.toEpochMilli()),
                record(1, "nsB", NOW.toEpochMilli()),
                record(2, "nsA", NOW.toEpochMilli()));

        SinkOutcome out = drain.drain(batch, NOW);

        assertEquals(SinkOutcome.Status.ALL_ACKED, out.status());
        assertEquals(2, sender.calls.get(), "two namespaces -> two PutMetricData calls");
        assertTrue(sender.namespaces.containsAll(List.of("nsA", "nsB")));
    }

    @Test
    void dropsStalePastDatumWithCounter() {
        CaptureSender sender = new CaptureSender();
        CloudWatchDrain drain = new CloudWatchDrain(sender);
        long old = NOW.minus(CloudWatchDrain.MAX_PAST).minusSeconds(60).toEpochMilli();
        List<SinkRecord> batch = List.of(
                record(0, "ns1", old),                    // stale -> dropped
                record(1, "ns1", NOW.toEpochMilli()));    // fresh -> sent

        SinkOutcome out = drain.drain(batch, NOW);

        assertEquals(SinkOutcome.Status.ALL_ACKED, out.status(), "dropped+sent both acked");
        assertEquals(1, drain.droppedStale());
        assertEquals(List.of(1), sender.chunkSizes, "only the fresh datum sent");
    }

    @Test
    void dropsStaleFutureDatumWithCounter() {
        CaptureSender sender = new CaptureSender();
        CloudWatchDrain drain = new CloudWatchDrain(sender);
        long future = NOW.plus(CloudWatchDrain.MAX_FUTURE).plusSeconds(600).toEpochMilli();
        List<SinkRecord> batch = List.of(record(0, "ns1", future));

        SinkOutcome out = drain.drain(batch, NOW);

        assertEquals(SinkOutcome.Status.ALL_ACKED, out.status());
        assertEquals(1, drain.droppedStale());
        assertEquals(0, sender.calls.get(), "nothing sent (only stale)");
    }

    @Test
    void dropsMalformedRecordWithCounter() {
        CaptureSender sender = new CaptureSender();
        CloudWatchDrain drain = new CloudWatchDrain(sender);
        SinkRecord bad = new SinkRecord(0, NOW.toEpochMilli(), "ns1", "garbage".getBytes());
        List<SinkRecord> batch = List.of(bad, record(1, "ns1", NOW.toEpochMilli()));

        SinkOutcome out = drain.drain(batch, NOW);

        assertEquals(SinkOutcome.Status.ALL_ACKED, out.status());
        assertEquals(1, drain.droppedMalformed());
        assertEquals(List.of(1), sender.chunkSizes);
    }

    @Test
    void chunksAtThousandDatumLimit() {
        CaptureSender sender = new CaptureSender();
        CloudWatchDrain drain = new CloudWatchDrain(sender);
        List<SinkRecord> batch = new ArrayList<>();
        for (int i = 0; i < 2500; i++) {
            batch.add(record(i, "ns1", NOW.toEpochMilli()));
        }

        SinkOutcome out = drain.drain(batch, NOW);

        assertEquals(SinkOutcome.Status.ALL_ACKED, out.status());
        // 2500 datums / 1000 -> 3 chunks (1000, 1000, 500)
        assertEquals(3, sender.calls.get());
        assertEquals(List.of(1000, 1000, 500), sender.chunkSizes);
    }

    /** A record whose serialized payload is large (~`approxBytes`) via a long dimension value. */
    private static SinkRecord bigRecord(long offset, String namespace, int approxBytes) {
        String big = "x".repeat(approxBytes);
        MetricDatum datum = MetricDatum.builder()
                .metricName("m").value((double) offset)
                .timestamp(NOW)
                .dimensions(software.amazon.awssdk.services.cloudwatch.model.Dimension.builder()
                        .name("blob").value(big).build())
                .build();
        return new SinkRecord(offset, NOW.toEpochMilli(), namespace,
                CloudWatchRecord.serialize(namespace, datum));
    }

    @Test
    void chunksAtByteLimit() {
        CaptureSender sender = new CaptureSender();
        CloudWatchDrain drain = new CloudWatchDrain(sender);
        // Each record ~100 KB -> the ~900 KB byte budget caps a chunk at ~8 datums, well under 1000.
        int perRecord = 100_000;
        List<SinkRecord> batch = new ArrayList<>();
        for (int i = 0; i < 20; i++) {
            batch.add(bigRecord(i, "ns1", perRecord));
        }

        SinkOutcome out = drain.drain(batch, NOW);

        assertEquals(SinkOutcome.Status.ALL_ACKED, out.status());
        assertTrue(sender.calls.get() >= 2, "byte budget forces multiple chunks before 1000 count");
        for (int size : sender.chunkSizes) {
            assertTrue(size < CloudWatchDrain.MAX_DATUMS_PER_REQUEST,
                    "each chunk is bounded by the byte limit, not the count limit");
            assertTrue(size >= 1);
        }
        // All 20 datums delivered across the chunks.
        assertEquals(20, sender.chunkSizes.stream().mapToInt(Integer::intValue).sum());
    }

    @Test
    void wholeBatchFailureIsRetryable() {
        CaptureSender sender = new CaptureSender();
        sender.failNamespaces = Set.of("ns1");
        CloudWatchDrain drain = new CloudWatchDrain(sender);
        List<SinkRecord> batch = List.of(
                record(0, "ns1", NOW.toEpochMilli()),
                record(1, "ns1", NOW.toEpochMilli()));

        SinkOutcome out = drain.drain(batch, NOW);

        assertEquals(SinkOutcome.Status.FAILED_RETRYABLE, out.status());
        assertTrue(out.failedOffsets().isEmpty());
    }

    @Test
    void partialFailureReportsFailedOffsets() {
        CaptureSender sender = new CaptureSender();
        sender.failNamespaces = Set.of("nsB");
        CloudWatchDrain drain = new CloudWatchDrain(sender);
        List<SinkRecord> batch = List.of(
                record(0, "nsA", NOW.toEpochMilli()),     // sent
                record(1, "nsB", NOW.toEpochMilli()),     // failed
                record(2, "nsB", NOW.toEpochMilli()));    // failed

        SinkOutcome out = drain.drain(batch, NOW);

        assertEquals(SinkOutcome.Status.PARTIAL, out.status());
        assertEquals(Set.of(1L, 2L), new HashSet<>(out.failedOffsets()));
    }

    @Test
    void allStaleStillAcks() {
        CaptureSender sender = new CaptureSender();
        CloudWatchDrain drain = new CloudWatchDrain(sender);
        long old = NOW.minus(CloudWatchDrain.MAX_PAST).minusSeconds(1).toEpochMilli();
        List<SinkRecord> batch = List.of(record(0, "ns1", old), record(1, "ns1", old));

        SinkOutcome out = drain.drain(batch, NOW);

        assertEquals(SinkOutcome.Status.ALL_ACKED, out.status());
        assertEquals(2, drain.droppedStale());
        assertEquals(0, sender.calls.get());
    }

    @Test
    void drainWithDefaultNowUsesWallClock() {
        CaptureSender sender = new CaptureSender();
        CloudWatchDrain drain = new CloudWatchDrain(sender);
        // A fresh "now" timestamp should be accepted by the no-arg drain (wall clock).
        SinkRecord fresh = record(0, "ns1", Instant.now().toEpochMilli());
        SinkOutcome out = drain.drain(List.of(fresh));
        assertEquals(SinkOutcome.Status.ALL_ACKED, out.status());
        assertEquals(1, sender.calls.get());
    }
}
