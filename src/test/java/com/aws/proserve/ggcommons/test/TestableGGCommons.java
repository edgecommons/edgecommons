/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.test;

import com.aws.proserve.ggcommons.GGCommons;
import com.aws.proserve.ggcommons.interfaces.IConfigurationService;
import com.aws.proserve.ggcommons.interfaces.IMessagingService;
import com.aws.proserve.ggcommons.interfaces.IMetricService;
import com.aws.proserve.ggcommons.metrics.MetricEmitter;

/**
 * Test-specific GGCommons that allows service injection before initialization.
 * This enables true unit testing by injecting mocks before any real services are created.
 */
public class TestableGGCommons extends GGCommons {
    
    public TestableGGCommons(String componentName, String[] args) {
        super(); // Use protected empty constructor
        
        try {
            // Initialize with mocks - this will create configManager and serviceRegistry
            initForTesting(componentName, args);
            
            // Initialize MetricEmitter for tests that create Metric objects directly
            MetricEmitter.init(getConfigManager());
            
            // Now override with our specific mock instances
            registerService(IMessagingService.class, new MockMessagingService());
            registerService(IMetricService.class, new MockMetricService());
        } catch (Exception e) {
            throw new RuntimeException("Failed to initialize TestableGGCommons: " + e.getMessage(), e);
        }
    }
}