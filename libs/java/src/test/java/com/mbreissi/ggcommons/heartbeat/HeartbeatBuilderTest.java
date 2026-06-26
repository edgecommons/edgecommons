/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.heartbeat;

import com.mbreissi.ggcommons.test.MockConfigurationService;
import com.mbreissi.ggcommons.test.MockMessagingService;
import com.mbreissi.ggcommons.test.MockMetricService;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Covers the validation branches of {@link HeartbeatBuilder} that the happy-path
 * tests don't reach: {@code create(null)} (L31-L32), {@code build()} with no
 * messaging service (L66-L67), and {@code build()} with no metric service (L69-L70).
 */
class HeartbeatBuilderTest {

    @Test
    void createWithNullConfigThrows() {
        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class,
                () -> HeartbeatBuilder.create(null));
        assertTrue(ex.getMessage().toLowerCase().contains("configuration"));
    }

    @Test
    void buildWithoutMessagingServiceThrows() {
        HeartbeatBuilder builder = HeartbeatBuilder.create(new MockConfigurationService())
                .withMetricService(new MockMetricService());
        // messagingService not set
        IllegalStateException ex = assertThrows(IllegalStateException.class, builder::build);
        assertTrue(ex.getMessage().toLowerCase().contains("messaging"));
    }

    @Test
    void buildWithoutMetricServiceThrows() {
        HeartbeatBuilder builder = HeartbeatBuilder.create(new MockConfigurationService())
                .withMessagingService(new MockMessagingService());
        // metricService not set
        IllegalStateException ex = assertThrows(IllegalStateException.class, builder::build);
        assertTrue(ex.getMessage().toLowerCase().contains("metric"));
    }
}
