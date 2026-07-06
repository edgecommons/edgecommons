/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import com.mbreissi.edgecommons.test.MockConfigurationService;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Regression tests for message/tag serialization round-trips.
 */
class MessageSerializationTest {

    /**
     * {@code MessageTags.toDict()} previously mutated the backing tags object (via the live
     * {@code JsonObject.asMap()} view), leaking synthesized keys into it on every call.
     */
    @Test
    void toDictDoesNotMutateInternalTags() {
        MessageTags tags = new MessageTags();
        tags.injectTag("a", "1");

        tags.toDict();

        assertFalse(tags.tags.has("thing"), "toDict() must not mutate the internal tags object");
        assertTrue(tags.tags.has("a"));
        assertEquals(1, tags.tags.size());
    }

    /**
     * {@code Message.build()} previously read the body with getAsJsonObject(), throwing on a
     * non-object (array/primitive) body. It must accept any JSON body, matching serialization.
     */
    @Test
    void buildDeserializesNonObjectBody() {
        JsonObject msg = new JsonObject();
        JsonObject header = new JsonObject();
        header.addProperty("name", "X");
        msg.add("header", header);
        JsonArray body = new JsonArray();
        body.add(1);
        body.add(2);
        msg.add("body", body);

        Message m = Message.build(msg);

        assertNotNull(m.getBody());
        assertTrue(((JsonElement) m.getBody()).isJsonArray());
        assertEquals(2, ((JsonElement) m.getBody()).getAsJsonArray().size());
    }

    /**
     * #16: a {@code byte[]} body must serialize as the first-class binary body marker, not as a
     * Gson number array or a lossy bare string.
     */
    @Test
    void byteArrayBodySerializesAsBinaryMarker() {
        byte[] bytes = new byte[] {0, 1, 2, (byte) 254, (byte) 255};

        Message m = MessageBuilder.create("Bin", "1.0").withPayload(bytes)
            .withConfig(new MockConfigurationService()).build();
        JsonElement body = m.toDict().get("body");

        JsonObject marker = body.getAsJsonObject().getAsJsonObject("_edgecommonsBinary");
        assertEquals("base64", marker.get("encoding").getAsString());
        assertEquals(5, marker.get("length").getAsInt());
        assertEquals("AAEC/v8=", marker.get("data").getAsString());
        assertTrue(m.isBinaryBody());
        assertArrayEquals(bytes, m.getBinaryBody());
    }

    @Test
    void inboundBinaryMarkerDecodesAndValidatesLength() {
        JsonObject marker = new JsonObject();
        JsonObject descriptor = new JsonObject();
        descriptor.addProperty("encoding", "base64");
        descriptor.addProperty("length", 5);
        descriptor.addProperty("data", "AAEC/v8=");
        marker.add("_edgecommonsBinary", descriptor);
        JsonObject msg = new JsonObject();
        msg.add("body", marker);

        Message m = MessageBuilder.fromObject(msg);

        assertTrue(m.isBinaryBody());
        assertArrayEquals(new byte[] {0, 1, 2, (byte) 254, (byte) 255}, m.getBinaryBody());
        descriptor.addProperty("length", 4);
        assertThrows(IllegalArgumentException.class, m::getBinaryBody);
    }

    @Test
    void oversizedBinaryBodyIsRejected() {
        byte[] bytes = new byte[Message.MAX_BINARY_BODY_BYTES + 1];
        Message m = MessageBuilder.create("Bin", "1.0").withPayload(bytes).build();

        assertThrows(IllegalArgumentException.class, m::toDict);
    }

    /**
     * #15: an explicit null-valued entry in a {@code Map} body must serialize as JSON {@code null}
     * (parity with Python dict None / TS object null / serde), not be dropped by Gson's default
     * null-omitting serializer.
     */
    @Test
    void mapBodyPreservesExplicitNullEntry() {
        java.util.Map<String, Object> nested = new java.util.LinkedHashMap<>();
        nested.put("inner", null);  // a NESTED null (toJsonTree drops these; the fix must not)
        java.util.Map<String, Object> payload = new java.util.LinkedHashMap<>();
        payload.put("present", 1);
        payload.put("nullv", null);
        payload.put("nested", nested);

        Message m = MessageBuilder.create("M", "1.0").withPayload(payload)
            .withConfig(new MockConfigurationService()).build();
        JsonObject body = m.toDict().get("body").getAsJsonObject();

        assertTrue(body.has("nullv"), "an explicit null Map entry must be preserved (#15)");
        assertTrue(body.get("nullv").isJsonNull());
        assertEquals(1, body.get("present").getAsInt());
        // The full wire string (JsonObject.toString) must also carry both nulls.
        assertTrue(body.getAsJsonObject("nested").has("inner"), "a NESTED null entry must survive too");
        assertTrue(body.getAsJsonObject("nested").get("inner").isJsonNull());
        assertTrue(m.toString().contains("\"inner\":null"), "nested null must reach the wire string");
    }
}
