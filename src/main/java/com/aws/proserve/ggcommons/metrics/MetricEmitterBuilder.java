/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.messaging.MessagingClient;

/**
 * Builder for creating MetricEmitter instances, wiring the configuration and messaging collaborators.
 */
public class MetricEmitterBuilder {
    private ConfigManager configurationService;
    private MessagingClient messagingService;

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

    public MetricEmitter build() {
        return new MetricEmitter(configurationService, messagingService);
    }
}
