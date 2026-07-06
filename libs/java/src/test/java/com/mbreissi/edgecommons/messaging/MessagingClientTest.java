/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import com.mbreissi.edgecommons.ParsedCommandLine;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import com.mbreissi.edgecommons.messaging.Qos;
import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for MessagingClient class.
 * Tests instance methods for messaging operations including IoT Core functionality.
 */
class MessagingClientTest {
    
    private MessagingClient messagingClient;
    
    @BeforeEach
    void setUp() {
        // Create a test ParsedCommandLine for IPC (GREENGRASS) transport
        ParsedCommandLine cmdLine = new ParsedCommandLine();
        cmdLine.transport = com.mbreissi.edgecommons.platform.Transport.IPC;
        
        // Create MessagingClient instance using builder
        messagingClient = MessagingClientBuilder.create(cmdLine)
                .withReceiveOwnMessages(true)
                .build();
    }

    @Test
    void testTopicMatchesFilter() {
        // Test single-level wildcard
        assertTrue(MessagingClient.topicMatchesFilter("sensors/+/temperature", "sensors/device1/temperature"));
        assertTrue(MessagingClient.topicMatchesFilter("test/+", "test/data"));
        assertFalse(MessagingClient.topicMatchesFilter("test/+", "other/data"));
        
        // Test multi-level wildcard
        assertTrue(MessagingClient.topicMatchesFilter("sensors/#", "sensors/device1/temperature"));
        assertTrue(MessagingClient.topicMatchesFilter("test/#", "test/data/value"));
        assertTrue(MessagingClient.topicMatchesFilter("test/#", "test/data/value/nested"));
        assertFalse(MessagingClient.topicMatchesFilter("test/#", "other/data"));
        
        // Test exact match
        assertTrue(MessagingClient.topicMatchesFilter("exact/topic", "exact/topic"));
        assertFalse(MessagingClient.topicMatchesFilter("exact/topic", "different/topic"));
    }

    @Test
    void testMessagingClientCreation() {
        assertNotNull(messagingClient);
        assertNotNull(messagingClient.getNativeLocalClient());
    }

    @Test
    void testPublishRaw() {
        JsonObject payload = new JsonObject();
        payload.addProperty("sensor", "temperature");
        payload.addProperty("value", 23.5);
        
        // This test just verifies the method can be called without exception
        // In a real test environment, you would mock the underlying provider
        assertDoesNotThrow(() -> {
            messagingClient.publishRaw("test/topic", payload);
        });
    }
}