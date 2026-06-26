/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.metrics;

import com.breissinger.ggcommons.config.ConfigManager;
import com.breissinger.ggcommons.messaging.MessagingClient;
import com.breissinger.ggcommons.platform.Platform;

/**
 * Builder for creating MetricEmitter instances, wiring the configuration and messaging collaborators.
 */
public class MetricEmitterBuilder {
    private ConfigManager configurationService;
    private MessagingClient messagingService;
    private Platform platform;

    private MetricEmitterBuilder(ConfigManager configurationService) {
        this.configurationService = configurationService;
    }

    public static MetricEmitterBuilder create(ConfigManager configurationService) {
        return new MetricEmitterBuilder(configurationService);
    }

    public MetricEmitterBuilder withMessagingService(MessagingClient messagingService) {
        this.messagingService = messagingService;
        return this;
    }

    /**
     * Sets the resolved deployment platform, which selects the platform-profile metric-target default
     * (prometheus on KUBERNETES) when the config omits {@code metricEmission.target} (FR-RT-3).
     *
     * @param platform the resolved platform, or {@code null} for none
     * @return this builder
     */
    public MetricEmitterBuilder withPlatform(Platform platform) {
        this.platform = platform;
        return this;
    }

    public MetricEmitter build() {
        return new MetricEmitter(configurationService, messagingService, platform);
    }
}
