/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.metrics.targets;

import software.amazon.awssdk.services.cloudwatch.model.MetricDatum;

import java.util.List;

/**
 * Sends one chunk of {@code MetricDatum} for a single namespace to CloudWatch
 * ({@code PutMetricData}). Abstracted so the durable drain can be unit-tested with a mocked /
 * fault-injecting sender (no real AWS), and so the production path can wrap an injected
 * {@code CloudWatchClient}.
 *
 * <p>The contract mirrors {@code PutMetricData}: a normal return means the whole chunk was accepted;
 * a thrown exception means it was not (the drain treats it as a retryable batch failure so the
 * durable buffer holds the records and the export engine retries on the next loop / reconnect).
 */
@FunctionalInterface
public interface CloudWatchSender {

    /**
     * Send one chunk (≤1000 datums, ≤~1 MB) under {@code namespace}.
     *
     * @throws RuntimeException if the request was not accepted (drives a retry)
     */
    void send(String namespace, List<MetricDatum> chunk);
}
