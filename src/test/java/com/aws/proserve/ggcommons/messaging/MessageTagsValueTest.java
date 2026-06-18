/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for {@link MessageTags} constructors, injectTag, and fromDict()/toDict()
 * round-trips. Complements MessageTagsBuilderTest by exercising the constructors and
 * the static fromDict() path directly.
 */
class MessageTagsValueTest {

    @Test
    void thingNameOnlyConstructorStartsEmpty() {
        MessageTags tags = new MessageTags("device-1");
        Map<String, JsonElement> dict = tags.toDict();
        assertEquals("device-1", dict.get("thing").getAsString());
        // Only the synthesized "thing" key is present.
        assertEquals(1, dict.size());
    }

    @Test
    void twoArgConstructorWrapsProvidedTags() {
        JsonObject backing = new JsonObject();
        backing.addProperty("a", "1");
        backing.addProperty("b", "2");

        MessageTags tags = new MessageTags("device-2", backing);
        Map<String, JsonElement> dict = tags.toDict();

        assertEquals("device-2", dict.get("thing").getAsString());
        assertEquals("1", dict.get("a").getAsString());
        assertEquals("2", dict.get("b").getAsString());
        assertEquals(3, dict.size());
    }

    @Test
    void injectTagAddsEntry() {
        MessageTags tags = new MessageTags("device-3");
        tags.injectTag("env", "prod");
        assertEquals("prod", tags.toDict().get("env").getAsString());
    }

    @Test
    void fromDictSeparatesThingFromTags() {
        JsonObject src = new JsonObject();
        src.addProperty("thing", "device-4");
        src.addProperty("region", "us-east-1");
        src.addProperty("service", "ingest");

        MessageTags tags = MessageTags.fromDict(src);
        Map<String, JsonElement> dict = tags.toDict();

        assertEquals("device-4", dict.get("thing").getAsString());
        assertEquals("us-east-1", dict.get("region").getAsString());
        assertEquals("ingest", dict.get("service").getAsString());
    }

    @Test
    void fromDictWithoutThingYieldsNullThing() {
        JsonObject src = new JsonObject();
        src.addProperty("only", "value");

        MessageTags tags = MessageTags.fromDict(src);
        Map<String, JsonElement> dict = tags.toDict();

        // A null thing name is omitted from the serialized form (so it round-trips back to null).
        assertFalse(dict.containsKey("thing"));
        assertEquals("value", dict.get("only").getAsString());
    }

    @Test
    void fromDictThenToDictRoundTripsTagValues() {
        JsonObject src = new JsonObject();
        src.addProperty("thing", "device-5");
        src.addProperty("k1", "v1");
        src.addProperty("k2", "v2");

        MessageTags tags = MessageTags.fromDict(src);

        // Re-serialize and confirm the non-thing entries survive intact.
        Map<String, JsonElement> dict = tags.toDict();
        assertEquals("v1", dict.get("k1").getAsString());
        assertEquals("v2", dict.get("k2").getAsString());
        assertEquals("device-5", dict.get("thing").getAsString());
    }

    @Test
    void toDictDoesNotMutateBackingTagsOnRepeatedCalls() {
        MessageTags tags = new MessageTags("device-6");
        tags.injectTag("a", "1");

        tags.toDict();
        tags.toDict();

        // Backing object must not accumulate a "thing" key across calls.
        assertFalse(tags.tags.has("thing"));
        assertEquals(1, tags.tags.size());
        assertEquals("1", tags.tags.get("a").getAsString());
    }

    @Test
    void toStringReflectsTags() {
        MessageTags tags = new MessageTags("device-7");
        tags.injectTag("env", "test");
        String s = tags.toString();
        assertTrue(s.contains("env"));
        assertTrue(s.contains("test"));
        assertTrue(s.contains("device-7"));
    }

    @Test
    void nullThingNameIsOmittedFromToDict() {
        MessageTags tags = new MessageTags(null);
        assertNull(tags.thingName);
        // A null thing name must not be serialized (would NPE as a JsonPrimitive); it is omitted.
        assertFalse(tags.toDict().containsKey("thing"));
    }
}
