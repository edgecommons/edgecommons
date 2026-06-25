/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.streaming;

import java.util.List;

/**
 * A host-provided sink for a {@code callback}-type ggstreamlog stream — the "bring-your-own-sink"
 * extension point. The export engine invokes {@link #send(List)} on its background thread with one
 * batch of {@link SinkRecord}s; the implementation drains them to its destination (CloudWatch
 * {@code PutMetricData}, a custom protocol, a local file, ...) and returns a {@link SinkOutcome}
 * following the at-least-once commit rules.
 *
 * <p>Implementations must be thread-safe and return promptly — the call blocks that stream's drain.
 * Throwing is tolerated (the bridge treats it as a retryable failure), but returning an explicit
 * outcome is preferred.
 */
@FunctionalInterface
public interface SinkFunction {

    /**
     * Drain one batch. The batch is never empty.
     *
     * @param batch the records to deliver (JVM-owned copies; safe to retain)
     * @return how the batch was handled (drives commit/retry in the engine)
     */
    SinkOutcome send(List<SinkRecord> batch);
}
