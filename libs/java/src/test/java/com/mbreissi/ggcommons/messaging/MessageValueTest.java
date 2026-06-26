/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.messaging;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.google.gson.JsonPrimitive;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for {@link Message} request preparation, correlation/tag mutation,
 * toDict()/toString() serialization, and the deprecated {@link Message#build(Object)}
 * factory. Complements MessageTest / MessageBuilderTest / MessageSerializationTest
 * with non-overlapping coverage.
 */
class MessageValueTest {

    private static JsonObject fullMessageJson() {
        JsonObject msg = new JsonObject();

        JsonObject header = new JsonObject();
        header.addProperty("name", "SensorReading");
        header.addProperty("version", "1.0");
        header.addProperty("correlation_id", "corr-1");
        header.addProperty("reply_to", "ggcommons/reply-abc");
        msg.add("header", header);

        JsonObject tags = new JsonObject();
        tags.addProperty("thing", "device-1");
        tags.addProperty("env", "prod");
        msg.add("tags", tags);

        JsonObject body = new JsonObject();
        body.addProperty("temp", 21);
        msg.add("body", body);

        return msg;
    }

    @Test
    void buildFromFullJsonPopulatesAllSections() {
        Message m = Message.build(fullMessageJson());

        assertNotNull(m.getHeader());
        assertEquals("SensorReading", m.getHeader().getName());
        assertEquals("1.0", m.getHeader().getVersion());
        assertEquals("corr-1", m.getCorrelationId());
        assertEquals("ggcommons/reply-abc", m.getHeader().getReplyTo());

        assertNotNull(m.getTags());
        assertEquals("device-1", m.getTags().toDict().get("thing").getAsString());
        assertEquals("prod", m.getTags().toDict().get("env").getAsString());

        assertNotNull(m.getBody());
        assertEquals(21, ((JsonElement) m.getBody()).getAsJsonObject().get("temp").getAsInt());
        assertNull(m.getRaw());
    }

    @Test
    void toDictAndToStringForFullMessage() {
        Message m = Message.build(fullMessageJson());

        JsonObject dict = m.toDict();
        assertTrue(dict.has("header"));
        assertTrue(dict.has("tags"));
        assertTrue(dict.has("body"));
        assertFalse(dict.has("raw"));
        assertEquals("device-1", dict.getAsJsonObject("tags").get("thing").getAsString());

        // toString() delegates to toDict().toString() and must be valid JSON.
        JsonObject reparsed = JsonParser.parseString(m.toString()).getAsJsonObject();
        assertEquals("SensorReading", reparsed.getAsJsonObject("header").get("name").getAsString());
    }

    @Test
    void buildFromJsonObjectWithNoKnownSectionsBecomesRaw() {
        // A JsonObject with none of header/tags/body keys is stored as raw.
        JsonObject unknown = new JsonObject();
        unknown.addProperty("foo", "bar");

        Message m = Message.build(unknown);

        assertNull(m.getHeader());
        assertNull(m.getTags());
        assertNull(m.getBody());
        assertNotNull(m.getRaw());
        assertEquals("bar", ((JsonElement) m.getRaw()).getAsJsonObject().get("foo").getAsString());

        // raw is a JsonElement here, so toDict() exercises the raw branch safely.
        JsonObject dict = m.toDict();
        assertTrue(dict.has("raw"));
        assertFalse(dict.has("body"));
    }

    @Test
    void buildFromNonJsonObjectBecomesRaw() {
        Message m = Message.build("just a plain string");

        assertNull(m.getHeader());
        assertEquals("just a plain string", m.getRaw());
    }

    @Test
    void buildFromMinimalHeaderOnly() {
        JsonObject msg = new JsonObject();
        JsonObject header = new JsonObject();
        header.addProperty("name", "OnlyHeader");
        header.addProperty("version", "2.0");
        msg.add("header", header);

        Message m = Message.build(msg);

        assertNotNull(m.getHeader());
        assertEquals("OnlyHeader", m.getHeader().getName());
        assertNull(m.getTags());
        assertNull(m.getBody());
        assertNull(m.getRaw());
    }

    @Test
    void getCorrelationIdNullWhenNoHeader() {
        Message m = Message.build("raw-payload");
        assertNull(m.getCorrelationId());
    }

    @Test
    void makeRequestWithExplicitReplyToCreatesHeaderWhenAbsent() {
        Message m = Message.build("raw-payload");
        assertNull(m.getHeader());

        String replyTo = m.makeRequest("my/reply/topic");

        assertEquals("my/reply/topic", replyTo);
        assertNotNull(m.getHeader());
        assertEquals("my/reply/topic", m.getHeader().getReplyTo());
        assertEquals("None", m.getHeader().getName());
        assertEquals("None", m.getHeader().getVersion());
    }

    @Test
    void makeRequestNoArgAutoGeneratesReplyTopic() {
        Message m = Message.build("raw-payload");

        String replyTo = m.makeRequest();

        assertNotNull(replyTo);
        assertTrue(replyTo.startsWith(MessageHeader.REPLY_MESSAGE_TOPIC_PREFIX),
                "auto reply topic must use the reply prefix");
        assertEquals(replyTo, m.getHeader().getReplyTo());
    }

    @Test
    void makeRequestUsesExistingHeader() {
        Message m = Message.build(fullMessageJson());
        String replyTo = m.makeRequest("override/reply");
        assertEquals("override/reply", replyTo);
        assertEquals("override/reply", m.getHeader().getReplyTo());
        // existing header preserved (name not overwritten with "None")
        assertEquals("SensorReading", m.getHeader().getName());
    }

    @Test
    void setCorrelationIdCreatesHeaderWhenAbsent() {
        Message m = Message.build("raw-payload");
        m.setCorrelationId("set-corr-1");
        assertNotNull(m.getHeader());
        assertEquals("set-corr-1", m.getCorrelationId());
    }

    @Test
    void setCorrelationIdUpdatesExistingHeader() {
        Message m = Message.build(fullMessageJson());
        m.setCorrelationId("set-corr-2");
        assertEquals("set-corr-2", m.getCorrelationId());
    }

    @Test
    void injectTagAddsToExistingTags() {
        Message m = Message.build(fullMessageJson());
        m.injectTag("extra", "value");
        assertEquals("value", m.getTags().toDict().get("extra").getAsString());
        // pre-existing tag still present
        assertEquals("prod", m.getTags().toDict().get("env").getAsString());
    }

    @Test
    void fromObjectAndBuildAgreeOnFullMessage() {
        JsonObject json = fullMessageJson();
        Message viaBuild = Message.build(json);
        Message viaFromObject = MessageBuilder.fromObject(json);

        assertEquals(viaBuild.getHeader().getName(), viaFromObject.getHeader().getName());
        assertEquals(viaBuild.getCorrelationId(), viaFromObject.getCorrelationId());
        assertEquals(
                viaBuild.getTags().toDict().get("env").getAsString(),
                viaFromObject.getTags().toDict().get("env").getAsString());
    }

    @Test
    void toDictBodyMatchesInjectedPrimitive() {
        // Exercise toDict() body branch with a primitive JsonElement body.
        JsonObject msg = new JsonObject();
        JsonObject header = new JsonObject();
        header.addProperty("name", "P");
        header.addProperty("version", "1.0");
        msg.add("header", header);
        msg.add("body", new JsonPrimitive(42));

        Message m = Message.build(msg);
        JsonObject dict = m.toDict();
        assertEquals(42, dict.get("body").getAsInt());
    }
}
