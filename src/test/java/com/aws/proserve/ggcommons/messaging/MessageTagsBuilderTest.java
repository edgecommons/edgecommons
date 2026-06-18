package com.aws.proserve.ggcommons.messaging;

import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;
import java.util.HashMap;
import java.util.Map;
import static org.junit.jupiter.api.Assertions.*;

class MessageTagsBuilderTest {

    @Test
    void testBasicBuilder() {
        MessageTags tags = MessageTagsBuilder.create("test-thing").build();
        
        assertNotNull(tags);
        assertEquals("test-thing", tags.toDict().get("thing").getAsString());
    }
    
    @Test
    void testAddSingleTag() {
        MessageTags tags = MessageTagsBuilder.create("test-thing")
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
        
        MessageTags tags = MessageTagsBuilder.create("test-thing")
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
        
        MessageTags tags = MessageTagsBuilder.create("test-thing")
                .addTags(jsonTags)
                .build();
        
        Map<String, com.google.gson.JsonElement> dict = tags.toDict();
        assertEquals("test-service", dict.get("service").getAsString());
        assertEquals("1.2.3", dict.get("version").getAsString());
    }
    
    @Test
    void testBuilderChaining() {
        MessageTags tags = MessageTagsBuilder.create("test-thing")
                .addTag("env", "test")
                .addTag("region", "us-east-1")
                .addTag("service", "my-service")
                .build();
        
        Map<String, com.google.gson.JsonElement> dict = tags.toDict();
        assertEquals(4, dict.size()); // 3 added + thing
        assertEquals("test", dict.get("env").getAsString());
    }
}