package com.mbreissi.edgecommons.messaging;

import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;
import java.util.HashMap;
import java.util.Map;
import static org.junit.jupiter.api.Assertions.*;

class MessageTagsBuilderTest {

    @Test
    void testBasicBuilder() {
        MessageTags tags = MessageTagsBuilder.create().build();

        assertNotNull(tags);
        // UNS hard cut: no synthesized "thing" tag — an empty builder yields empty tags.
        assertTrue(tags.toDict().isEmpty());
    }

    @Test
    void testAddSingleTag() {
        MessageTags tags = MessageTagsBuilder.create()
                .addTag("environment", "production")
                .build();

        Map<String, com.google.gson.JsonElement> dict = tags.toDict();
        assertEquals("production", dict.get("environment").getAsString());
    }

    @Test
    void testAddMultipleTags() {
        var tagMap = new HashMap<String, String>();
        tagMap.put("env", "prod");
        tagMap.put("region", "us-west-2");

        MessageTags tags = MessageTagsBuilder.create()
                .addTags(tagMap)
                .build();

        Map<String, com.google.gson.JsonElement> dict = tags.toDict();
        assertEquals("prod", dict.get("env").getAsString());
        assertEquals("us-west-2", dict.get("region").getAsString());
    }

    @Test
    void testAddJsonTags() {
        var jsonTags = new JsonObject();
        jsonTags.addProperty("service", "test-service");
        jsonTags.addProperty("version", "1.2.3");

        MessageTags tags = MessageTagsBuilder.create()
                .addTags(jsonTags)
                .build();

        Map<String, com.google.gson.JsonElement> dict = tags.toDict();
        assertEquals("test-service", dict.get("service").getAsString());
        assertEquals("1.2.3", dict.get("version").getAsString());
    }

    @Test
    void testBuilderChaining() {
        MessageTags tags = MessageTagsBuilder.create()
                .addTag("env", "test")
                .addTag("region", "us-east-1")
                .addTag("service", "my-service")
                .build();

        Map<String, com.google.gson.JsonElement> dict = tags.toDict();
        assertEquals(3, dict.size());
        assertEquals("test", dict.get("env").getAsString());
    }
}
