/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.messaging;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for {@link MessageTags} constructors, injectTag, and fromDict()/toDict()
 * round-trips. Complements MessageTagsBuilderTest by exercising the constructors and
 * the static fromDict() path directly.
 *
 * <p>UNS hard cut: there is no synthesized {@code thing} tag anymore — the device travels in
 * the top-level {@code identity} envelope element. A stray inbound {@code thing} key is an
 * ordinary tag.
 */
class MessageTagsValueTest {

    @Test
    void noArgConstructorStartsEmpty() {
        MessageTags tags = new MessageTags();
        Map<String, JsonElement> dict = tags.toDict();
        assertTrue(dict.isEmpty());
    }

    @Test
    void jsonObjectConstructorWrapsProvidedTags() {
        JsonObject backing = new JsonObject();
        backing.addProperty("a", "1");
        backing.addProperty("b", "2");

        MessageTags tags = new MessageTags(backing);
        Map<String, JsonElement> dict = tags.toDict();

        assertEquals("1", dict.get("a").getAsString());
        assertEquals("2", dict.get("b").getAsString());
        assertEquals(2, dict.size());
    }

    @Test
    void injectTagAddsEntry() {
        MessageTags tags = new MessageTags();
        tags.injectTag("env", "prod");
        assertEquals("prod", tags.toDict().get("env").getAsString());
    }

    @Test
    void fromDictTreatsStrayThingKeyAsOrdinaryTag() {
        // UNS hard cut: no "thing" special-casing — a stray inbound thing key just lands
        // in the generic tag map.
        JsonObject src = new JsonObject();
        src.addProperty("thing", "device-4");
        src.addProperty("region", "us-east-1");
        src.addProperty("service", "ingest");

        MessageTags tags = MessageTags.fromDict(src);
        Map<String, JsonElement> dict = tags.toDict();

        assertEquals("device-4", dict.get("thing").getAsString());
        assertEquals("us-east-1", dict.get("region").getAsString());
        assertEquals("ingest", dict.get("service").getAsString());
        assertEquals(3, dict.size());
    }

    @Test
    void fromDictThenToDictRoundTripsTagValues() {
        JsonObject src = new JsonObject();
        src.addProperty("k1", "v1");
        src.addProperty("k2", "v2");

        MessageTags tags = MessageTags.fromDict(src);

        // Re-serialize and confirm the entries survive intact.
        Map<String, JsonElement> dict = tags.toDict();
        assertEquals("v1", dict.get("k1").getAsString());
        assertEquals("v2", dict.get("k2").getAsString());
        assertEquals(2, dict.size());
    }

    @Test
    void toDictDoesNotMutateBackingTagsOnRepeatedCalls() {
        MessageTags tags = new MessageTags();
        tags.injectTag("a", "1");

        tags.toDict();
        tags.toDict();

        // Backing object must not accumulate keys across calls.
        assertEquals(1, tags.tags.size());
        assertEquals("1", tags.tags.get("a").getAsString());
    }

    @Test
    void toStringReflectsTags() {
        MessageTags tags = new MessageTags();
        tags.injectTag("env", "test");
        String s = tags.toString();
        assertTrue(s.contains("env"));
        assertTrue(s.contains("test"));
    }
}
