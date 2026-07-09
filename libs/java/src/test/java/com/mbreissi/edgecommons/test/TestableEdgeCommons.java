/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.test;

import com.mbreissi.edgecommons.EdgeCommons;
import com.mbreissi.edgecommons.ParsedCommandLine;
import com.mbreissi.edgecommons.config.ConfigManagerFactory;
import com.mbreissi.edgecommons.logging.LogService;

/**
 * Test-specific EdgeCommons that wires a real (file-backed) ConfigManager together with mock
 * messaging and metric collaborators, without standing up real Greengrass IPC / brokers.
 * This enables true unit testing of components that depend on a EdgeCommons instance.
 */
public class TestableEdgeCommons extends EdgeCommons {

    public TestableEdgeCommons(String componentName, String[] args) {
        super(); // protected no-arg constructor
        try {
            ParsedCommandLine parsedCommandLine = EdgeCommons.processArgs(componentName, args, null);
            // Real config manager (use a FILE config source so no messaging/IPC is required).
            this.configManager = ConfigManagerFactory.create(componentName, parsedCommandLine);
            // Mock collaborators injected directly - no real provider is created.
            this.messagingClient = new MockMessagingService();
            this.metricEmitter = new MockMetricService();
            this.logService = new LogService(this.configManager, this.messagingClient);
            this.configManager.addConfigChangeListener(this.logService);
            this.configManager.completeInitialization();
        } catch (Exception e) {
            throw new RuntimeException("Failed to initialize TestableEdgeCommons: " + e.getMessage(), e);
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
