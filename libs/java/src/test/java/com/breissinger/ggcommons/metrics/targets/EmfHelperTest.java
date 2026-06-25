/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.metrics.targets;

import com.breissinger.ggcommons.metrics.Metric;
import com.breissinger.ggcommons.metrics.MetricBuilder;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Regression tests for the EMF (CloudWatch Embedded Metric Format) helper.
 */
class EmfHelperTest {

    /**
     * The EMF spec requires {@code _aws.Timestamp} to be epoch MILLISECONDS. A prior bug
     * divided by 1000, emitting seconds, which lands metrics ~50 years in the past.
     */
    @Test
    void emfTimestampIsMilliseconds() {
        Metric metric = MetricBuilder.create("test")
                .addMeasure("val", "Count", 1)
                .withNamespace("ggcommons")
                .addDimension("component", "c")
                .build();

        long before = System.currentTimeMillis();
        JsonObject emf = EmfHelper.buildMetricData("ggcommons", metric, Map.of("val", 1.0f), false);
        long after = System.currentTimeMillis();

        long ts = emf.getAsJsonObject("_aws").get("Timestamp").getAsLong();
        assertTrue(ts >= before && ts <= after,
                "EMF _aws.Timestamp must be the current time in epoch milliseconds, got " + ts);
        assertTrue(ts > 1_000_000_000_000L,
                "EMF _aws.Timestamp must be milliseconds (>1e12), not seconds, got " + ts);
    }
}
