/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging;

import com.aws.proserve.ggcommons.test.MockConfigurationService;
import com.aws.proserve.ggcommons.test.TestableGGCommons;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for Message class.
 * Tests message creation, tag injection, and configuration-based building.
 */
class MessageTest {
    
    private static TestableGGCommons ggCommons;
    private static MockConfigurationService mockConfigService;
    
    @BeforeEach
    void setUp() {
        if (ggCommons == null) {
            mockConfigService = new MockConfigurationService();
            mockConfigService.setThingName("test-thing");
            mockConfigService.setComponentName("TestComponent");
            
            String[] args = {"-t", "test-thing", "-c", "FILE", "./config.json"};
            ggCommons = new TestableGGCommons("test.component", args);
        }
    }

    @Test
    void testInjectTag() {
        JsonObject payload = new JsonObject();
        payload.addProperty("test", "value");
        
        Message message = Message.build(payload);
        message.injectTag("environment", "production");
        message.injectTag("service", "test-service");
        
        assertNotNull(message);
        assertNotNull(message.getTags());
    }
    
    @Test
    void testInjectTagOnMessageWithoutTags() {
        JsonObject payload = new JsonObject();
        payload.addProperty("data", "test");
        
        Message message = Message.build(payload);
        assertNull(message.getTags()); // Initially no tags
        
        message.injectTag("region", "us-west-2");
        
        assertNotNull(message.getTags()); // Tags created after injection
    }

    @Test
    void testShutdownDoesNotThrow() {
        TestableGGCommons local = new TestableGGCommons("test.component",
                new String[]{"-t", "test-thing", "-c", "FILE", "./config.json"});
        assertDoesNotThrow(local::shutdown);
    }

    @Test
    void testBuildFromConfigWithCorrelationId() {
        JsonObject payload = new JsonObject();
        payload.addProperty("data", "test");
        payload.addProperty("timestamp", System.currentTimeMillis());
        
        Message message = Message.buildFromConfig("TestMessage", "1.0", payload, 
                ggCommons.getConfigManager(), "test-correlation-123");
        
        assertNotNull(message);
        assertNotNull(message.getHeader());
        assertEquals("TestMessage", message.getHeader().getName());
        assertEquals("1.0", message.getHeader().getVersion());
        assertEquals("test-correlation-123", message.getHeader().getCorrelationId());
        assertNotNull(message.getBody());
    }
    
    @Test
    void testBuildFromConfigWithStringPayload() {
        String stringPayload = "Simple string message";
        
        Message message = Message.buildFromConfig("StringMessage", "2.0", stringPayload, 
                ggCommons.getConfigManager(), "string-correlation");
        
        assertNotNull(message);
        assertEquals("StringMessage", message.getHeader().getName());
        assertEquals("2.0", message.getHeader().getVersion());
        assertEquals("string-correlation", message.getHeader().getCorrelationId());
        assertEquals(stringPayload, message.getBody());
    }
    
    @Test
    void testBuildFromConfigWithJsonStringPayload() {
        String jsonStringPayload = "{\"key\": \"value\", \"number\": 42}";
        
        Message message = Message.buildFromConfig("JsonStringMessage", "1.5", jsonStringPayload, 
                ggCommons.getConfigManager(), "json-correlation");
        
        assertNotNull(message);
        assertEquals("JsonStringMessage", message.getHeader().getName());
        assertEquals("1.5", message.getHeader().getVersion());
        assertEquals("json-correlation", message.getHeader().getCorrelationId());
        assertNotNull(message.getBody());
    }
}