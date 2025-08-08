/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.heartbeat;

import com.aws.proserve.ggcommons.interfaces.IConfigurationService;
import com.aws.proserve.ggcommons.interfaces.IMessagingService;
import com.aws.proserve.ggcommons.interfaces.IMetricService;

/**
 * Builder for creating Heartbeat instances with proper dependency injection.
 * Ensures all required services are provided before initialization.
 */
public class HeartbeatBuilder {
    private final IConfigurationService configurationService;
    private IMessagingService messagingService;
    private IMetricService metricService;
    
    private HeartbeatBuilder(IConfigurationService configurationService) {
        this.configurationService = configurationService;
    }
    
    /**
     * Creates a new HeartbeatBuilder with the required configuration service.
     * 
     * @param configurationService The configuration service (required)
     * @return A new HeartbeatBuilder instance
     */
    public static HeartbeatBuilder create(IConfigurationService configurationService) {
        if (configurationService == null) {
            throw new IllegalArgumentException("Configuration service cannot be null");
        }
        return new HeartbeatBuilder(configurationService);
    }
    
    /**
     * Sets the messaging service for the heartbeat.
     * 
     * @param messagingService The messaging service implementation
     * @return This builder instance for method chaining
     */
    public HeartbeatBuilder withMessagingService(IMessagingService messagingService) {
        this.messagingService = messagingService;
        return this;
    }
    
    /**
     * Sets the metric service for the heartbeat.
     * 
     * @param metricService The metric service implementation
     * @return This builder instance for method chaining
     */
    public HeartbeatBuilder withMetricService(IMetricService metricService) {
        this.metricService = metricService;
        return this;
    }
    
    /**
     * Builds and initializes the Heartbeat instance.
     * 
     * @return A fully initialized Heartbeat instance
     * @throws IllegalStateException if required services are not provided
     */
    public Heartbeat build() {
        if (messagingService == null) {
            throw new IllegalStateException("Messaging service is required");
        }
        if (metricService == null) {
            throw new IllegalStateException("Metric service is required");
        }
        
        return new Heartbeat(configurationService, messagingService, metricService);
    }
}