/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.heartbeat;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.aws.proserve.ggcommons.metrics.MetricEmitter;

/**
 * Builder for creating Heartbeat instances, wiring the required collaborators.
 * Ensures all required services are provided before initialization.
 */
public class HeartbeatBuilder {
    private final ConfigManager configurationService;
    private MessagingClient messagingService;
    private MetricEmitter metricService;

    private HeartbeatBuilder(ConfigManager configurationService) {
        this.configurationService = configurationService;
    }

    /**
     * Creates a new HeartbeatBuilder with the required configuration manager.
     *
     * @param configurationService The configuration manager (required)
     * @return A new HeartbeatBuilder instance
     */
    public static HeartbeatBuilder create(ConfigManager configurationService) {
        if (configurationService == null) {
            throw new IllegalArgumentException("Configuration manager cannot be null");
        }
        return new HeartbeatBuilder(configurationService);
    }

    /**
     * Sets the messaging client for the heartbeat.
     *
     * @param messagingService The messaging client
     * @return This builder instance for method chaining
     */
    public HeartbeatBuilder withMessagingService(MessagingClient messagingService) {
        this.messagingService = messagingService;
        return this;
    }

    /**
     * Sets the metric emitter for the heartbeat.
     *
     * @param metricService The metric emitter
     * @return This builder instance for method chaining
     */
    public HeartbeatBuilder withMetricService(MetricEmitter metricService) {
        this.metricService = metricService;
        return this;
    }

    /**
     * Builds and initializes the Heartbeat instance.
     *
     * @return A fully initialized Heartbeat instance
     * @throws IllegalStateException if required collaborators are not provided
     */
    public Heartbeat build() {
        if (messagingService == null) {
            throw new IllegalStateException("Messaging client is required");
        }
        if (metricService == null) {
            throw new IllegalStateException("Metric emitter is required");
        }

        return new Heartbeat(configurationService, messagingService, metricService);
    }
}
