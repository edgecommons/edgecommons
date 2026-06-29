/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.messaging;

import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for MessageBuilder class.
 * Tests the builder pattern methods for creating Message instances.
 */
class MessageBuilderTest {

    @Test
    void testBuilderWithCorrelationId() {
        MessageBuilder builder = MessageBuilder.create("TestMessage", "1.0")
                .withCorrelationId("test-correlation-id");
        
        assertNotNull(builder);
    }
    
    @Test
    void testBuilderWithPayload() {
        JsonObject payload = new JsonObject();
        payload.addProperty("data", "test-value");
        payload.addProperty("timestamp", System.currentTimeMillis());
        
        MessageBuilder builder = MessageBuilder.create("TestMessage", "1.0")
                .withPayload(payload);
        
        assertNotNull(builder);
    }
    
    @Test
    void testBuilderWithStringPayload() {
        String stringPayload = "Simple string payload";
        
        MessageBuilder builder = MessageBuilder.create("TestMessage", "1.0")
                .withPayload(stringPayload);
        
        assertNotNull(builder);
    }
    
    @Test
    void testBuilderChaining() {
        JsonObject payload = new JsonObject();
        payload.addProperty("test", "value");
        
        MessageBuilder builder = MessageBuilder.create("TestMessage", "1.0")
                .withCorrelationId("test-correlation-id")
                .withPayload(payload);
        
        assertNotNull(builder);
    }
}