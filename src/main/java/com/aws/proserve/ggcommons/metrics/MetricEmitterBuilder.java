/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics;

import com.aws.proserve.ggcommons.interfaces.IConfigurationService;
import com.aws.proserve.ggcommons.interfaces.IMessagingService;

/**
 * Builder for creating MetricEmitter instances with proper dependency injection.
 */
public class MetricEmitterBuilder {
    private IConfigurationService configurationService;
    private IMessagingService messagingService;
    
    private MetricEmitterBuilder(IConfigurationService configurationService) {
        this.configurationService = configurationService;
    }
    
    public static MetricEmitterBuilder create(IConfigurationService configurationService) {
        return new MetricEmitterBuilder(configurationService);
    }
    
    public MetricEmitterBuilder withMessagingService(IMessagingService messagingService) {
        this.messagingService = messagingService;
        return this;
    }
    
    public MetricEmitter build() {
        return new MetricEmitter(configurationService, messagingService);
    }
}