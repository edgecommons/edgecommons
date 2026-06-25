/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.messaging;

import com.breissinger.ggcommons.test.MockConfigurationService;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for {@link MessageBuilder#build()} payload handling branches:
 * a String payload that parses as JSON, a String payload that is plain text
 * (parse-fallback), a JSON-object payload, and the missing-config error path.
 * MessageBuilderTest only exercises the fluent setters; these drive build().
 */
class MessageBuilderBuildTest {

    private MockConfigurationService config;

    @BeforeEach
    void setUp() {
        config = new MockConfigurationService();
    }

    @Test
    void buildWithoutConfigThrows() {
        MessageBuilder builder = MessageBuilder.create("NoConfig", "1.0")
                .withPayload("hello");
        assertThrows(IllegalStateException.class, builder::build);
    }

    @Test
    void buildWithJsonStringPayloadParsesToObject() {
        Message message = MessageBuilder.create("JsonStr", "1.0")
                .withPayload("{\"k\":\"v\",\"n\":7}")
                .withConfig(config)
                .build();

        assertNotNull(message.getBody());
        // Gson parses the JSON string into a Map (Object.class).
        assertTrue(message.getBody() instanceof Map);
        @SuppressWarnings("unchecked")
        Map<String, Object> body = (Map<String, Object>) message.getBody();
        assertEquals("v", body.get("k"));
    }

    @Test
    void buildWithPlainStringPayloadKeepsStringOnParseFallback() {
        // Multiple unquoted tokens cannot form a single JSON value, so Gson throws and
        // the builder falls back to storing the raw String unchanged.
        String plain = "plain text not json";
        Message message = MessageBuilder.create("PlainStr", "1.0")
                .withPayload(plain)
                .withConfig(config)
                .build();

        assertEquals(plain, message.getBody());
    }

    @Test
    void buildWithJsonObjectPayloadStoredAsIs() {
        JsonObject payload = new JsonObject();
        payload.addProperty("data", "value");

        Message message = MessageBuilder.create("JsonObj", "1.0")
                .withPayload(payload)
                .withConfig(config)
                .build();

        assertSame(payload, message.getBody());
    }

    @Test
    void buildWithCorrelationIdSetsHeaderCorrelationId() {
        Message message = MessageBuilder.create("WithCorr", "1.0")
                .withCorrelationId("corr-123")
                .withPayload("text")
                .withConfig(config)
                .build();

        assertEquals("corr-123", message.getCorrelationId());
        assertNotNull(message.getTags());
    }
}
