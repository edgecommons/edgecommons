/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.messaging;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Regression tests for message/tag serialization round-trips.
 */
class MessageSerializationTest {

    /**
     * {@code MessageTags.toDict()} previously mutated the backing tags object (via the live
     * {@code JsonObject.asMap()} view), leaking a "thing" key into it on every call.
     */
    @Test
    void toDictDoesNotMutateInternalTags() {
        MessageTags tags = new MessageTags("thing-1");
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
}
