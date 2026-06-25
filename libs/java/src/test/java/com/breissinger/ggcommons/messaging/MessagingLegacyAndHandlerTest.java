/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.messaging;

import com.breissinger.ggcommons.config.ConfigManager;
import com.breissinger.ggcommons.config.TagConfiguration;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.when;

/**
 * Exercises the still-supported legacy / deprecated construction paths and the
 * {@link MessageHandler} default {@code accept} adapter:
 * <ul>
 *   <li>{@link Message#buildFromConfig(String, String, Object, ConfigManager, String)} (5-arg, with
 *       both a stringified-JSON payload and a plain-string payload),</li>
 *   <li>the deprecated 2-arg and 3-arg {@link MessageHeader} constructors,</li>
 *   <li>{@link MessageTags#fromConfig} with no {@link TagConfiguration} present (else branch), and</li>
 *   <li>{@link MessageHandler#accept(String, Message)} delegating to {@code handle}.</li>
 * </ul>
 */
class MessagingLegacyAndHandlerTest {

    private static ConfigManager configWithoutTags() {
        ConfigManager cm = mock(ConfigManager.class);
        when(cm.getTagConfig()).thenReturn(null);
        when(cm.getThingName()).thenReturn("thing-A");
        return cm;
    }

    @SuppressWarnings("deprecation")
    @Test
    void buildFromConfigFiveArgWithStringifiedJsonPayloadParsesBody() {
        ConfigManager cm = configWithoutTags();
        Message m = Message.buildFromConfig("Evt", "2.0", "{\"k\":\"v\"}", cm, "corr-123");

        assertEquals("corr-123", m.getCorrelationId());
        assertEquals("Evt", m.getHeader().getName());
        assertEquals("2.0", m.getHeader().getVersion());
        // A stringified JSON object payload is parsed into a generic object (gson LinkedTreeMap).
        assertInstanceOf(java.util.Map.class, m.getBody());
        assertEquals("v", ((java.util.Map<?, ?>) m.getBody()).get("k"));
    }

    @SuppressWarnings("deprecation")
    @Test
    void buildFromConfigFiveArgWithPlainStringKeepsString() {
        ConfigManager cm = configWithoutTags();
        Message m = Message.buildFromConfig("Evt", "2.0", "just text", cm, null);

        // null correlationId => header generates one
        assertNotNull(m.getCorrelationId());
        // A plain (non-JSON) string payload is kept verbatim as the body.
        assertEquals("just text", m.getBody());
    }

    @SuppressWarnings("deprecation")
    @Test
    void buildFromConfigFourArgDelegatesToBuilder() {
        ConfigManager cm = configWithoutTags();
        JsonObject payload = new JsonObject();
        payload.addProperty("a", "b");
        // 4-arg overload (no correlationId) -> MessageBuilder path; header gets a generated corr id.
        Message m = Message.buildFromConfig("Evt", "3.0", payload, cm);

        assertEquals("Evt", m.getHeader().getName());
        assertEquals("3.0", m.getHeader().getVersion());
        assertNotNull(m.getCorrelationId());
        assertEquals("b", m.toDict().getAsJsonObject("body").get("a").getAsString());
        // no tag config -> only the thing tag is present
        assertEquals("thing-A", m.getTags().toDict().get("thing").getAsString());
    }

    @SuppressWarnings("deprecation")
    @Test
    void buildFromConfigFiveArgWithNonStringPayload() {
        ConfigManager cm = configWithoutTags();
        JsonObject payload = new JsonObject();
        payload.addProperty("n", 5);
        Message m = Message.buildFromConfig("Evt", "1.0", payload, cm, "c1");

        assertEquals("c1", m.getCorrelationId());
        assertEquals(5, m.toDict().getAsJsonObject("body").get("n").getAsInt());
    }

    @SuppressWarnings("deprecation")
    @Test
    void deprecatedTwoArgHeaderConstructorAppliesDefaults() {
        MessageHeader h = new MessageHeader("Name", "1.0");
        assertEquals("Name", h.getName());
        assertEquals("1.0", h.getVersion());
        assertNotNull(h.getCorrelationId());  // auto-generated
        assertNotNull(h.getTimestamp());      // auto-generated
        assertNull(h.getReplyTo());
    }

    @SuppressWarnings("deprecation")
    @Test
    void deprecatedThreeArgHeaderConstructorKeepsCorrelationId() {
        MessageHeader h = new MessageHeader("Name", "1.0", "fixed-corr");
        assertEquals("fixed-corr", h.getCorrelationId());
        assertNotNull(h.getTimestamp());
    }

    @Test
    void tagsFromConfigWithNoTagConfigUsesEmptyTags() {
        ConfigManager cm = configWithoutTags();
        MessageTags tags = MessageTags.fromConfig(cm);

        // thing is present, but no other tags
        assertEquals("thing-A", tags.toDict().get("thing").getAsString());
        assertEquals(1, tags.toDict().size());
    }

    @Test
    void tagsFromConfigWithTagConfigCopiesTags() {
        ConfigManager cm = mock(ConfigManager.class);
        TagConfiguration tagConfig = mock(TagConfiguration.class);
        JsonObject td = new JsonObject();
        td.addProperty("env", "prod");
        when(tagConfig.toDict()).thenReturn(td);
        when(cm.getTagConfig()).thenReturn(tagConfig);
        when(cm.getThingName()).thenReturn("thing-B");

        MessageTags tags = MessageTags.fromConfig(cm);
        assertEquals("prod", tags.toDict().get("env").getAsString());
        assertEquals("thing-B", tags.toDict().get("thing").getAsString());
    }

    @Test
    void messageHandlerAcceptDelegatesToHandle() {
        AtomicReference<String> seenTopic = new AtomicReference<>();
        AtomicReference<Message> seenMsg = new AtomicReference<>();
        MessageHandler handler = (topic, message) -> {
            seenTopic.set(topic);
            seenMsg.set(message);
        };

        Message m = MessageBuilder.fromObject("payload");
        // call through the BiConsumer default method, which must route to handle()
        handler.accept("some/topic", m);

        assertEquals("some/topic", seenTopic.get());
        assertSame(m, seenMsg.get());
    }
}
