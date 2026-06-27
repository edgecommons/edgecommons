/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.messaging;

import com.mbreissi.ggcommons.test.MockConfigurationService;
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

    // ----- #13: non-JsonElement payloads serialize via Gson (no ClassCastException at publish) -----

    @Test
    void mapPayloadSerializesViaToDictWithoutClassCast() {
        Map<String, Object> payload = new java.util.LinkedHashMap<>();
        payload.put("from", "java");
        payload.put("seq", 1);

        Message message = MessageBuilder.create("MapEvt", "1.0")
                .withPayload(payload)
                .withConfig(config)
                .build();

        // Previously threw: LinkedHashMap cannot be cast to JsonElement.
        JsonObject dict = assertDoesNotThrow(message::toDict);
        JsonObject body = dict.getAsJsonObject("body");
        assertEquals("java", body.get("from").getAsString());
        assertEquals(1, body.get("seq").getAsInt());
    }

    @Test
    void jsonStringPayloadSerializesViaToDict() {
        // build() parses a JSON string into a Map(Object.class); toDict() must still serialize it.
        Message message = MessageBuilder.create("JsonStr2", "1.0")
                .withPayload("{\"k\":\"v\",\"n\":7}")
                .withConfig(config)
                .build();

        JsonObject body = assertDoesNotThrow(message::toDict).getAsJsonObject("body");
        assertEquals("v", body.get("k").getAsString());
        assertEquals(7, body.get("n").getAsInt());
    }

    @Test
    void jsonObjectPayloadSerializesViaToDict() {
        JsonObject payload = new JsonObject();
        payload.addProperty("data", "value");

        Message message = MessageBuilder.create("JsonObj2", "1.0")
                .withPayload(payload)
                .withConfig(config)
                .build();

        assertEquals("value", message.toDict().getAsJsonObject("body").get("data").getAsString());
    }

    @Test
    void pojoPayloadSerializesViaToDict() {
        record Point(int x, int y) {}

        Message message = MessageBuilder.create("Pojo", "1.0")
                .withPayload(new Point(3, 4))
                .withConfig(config)
                .build();

        JsonObject body = message.toDict().getAsJsonObject("body");
        assertEquals(3, body.get("x").getAsInt());
        assertEquals(4, body.get("y").getAsInt());
    }
}
