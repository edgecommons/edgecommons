/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.test;

import com.mbreissi.ggcommons.GGCommons;
import com.mbreissi.ggcommons.ParsedCommandLine;
import com.mbreissi.ggcommons.config.ConfigManagerFactory;

/**
 * Test-specific GGCommons that wires a real (file-backed) ConfigManager together with mock
 * messaging and metric collaborators, without standing up real Greengrass IPC / brokers.
 * This enables true unit testing of components that depend on a GGCommons instance.
 */
public class TestableGGCommons extends GGCommons {

    public TestableGGCommons(String componentName, String[] args) {
        super(); // protected no-arg constructor
        try {
            ParsedCommandLine parsedCommandLine = GGCommons.processArgs(componentName, args, null);
            // Real config manager (use a FILE config source so no messaging/IPC is required).
            this.configManager = ConfigManagerFactory.create(componentName, parsedCommandLine);
            // Mock collaborators injected directly - no real provider is created.
            this.messagingClient = new MockMessagingService();
            this.metricEmitter = new MockMetricService();
            this.configManager.completeInitialization();
        } catch (Exception e) {
            throw new RuntimeException("Failed to initialize TestableGGCommons: " + e.getMessage(), e);
        }
    }

    /** Convenience accessor returning the injected messaging mock. */
    public MockMessagingService getMockMessaging() {
        return (MockMessagingService) getMessaging();
    }

    /** Convenience accessor returning the injected metric mock. */
    public MockMetricService getMockMetrics() {
        return (MockMetricService) getMetrics();
    }
}
