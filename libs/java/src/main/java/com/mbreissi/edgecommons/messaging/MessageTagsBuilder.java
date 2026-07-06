package com.mbreissi.edgecommons.messaging;

import com.google.gson.JsonObject;
import java.util.Map;

/**
 * Builder for creating MessageTags instances with fluent API.
 *
 * <p>UNS hard cut: the thing name is no longer a tag concern — the device travels in the
 * top-level {@code identity} envelope element, so this builder only assembles business tags.
 */
public class MessageTagsBuilder {
    private JsonObject tags = new JsonObject();

    private MessageTagsBuilder() {
    }

    public static MessageTagsBuilder create() {
        return new MessageTagsBuilder();
    }

    public MessageTagsBuilder addTag(String key, String value) {
        if (key != null && value != null) {
            tags.addProperty(key, value);
        }
        return this;
    }

    public MessageTagsBuilder addTags(Map<String, String> tagMap) {
        if (tagMap != null) {
            for (Map.Entry<String, String> entry : tagMap.entrySet()) {
                if (entry.getKey() != null && entry.getValue() != null) {
                    tags.addProperty(entry.getKey(), entry.getValue());
                }
            }
        }
        return this;
    }

    public MessageTagsBuilder addTags(JsonObject tagObject) {
        if (tagObject != null) {
            for (Map.Entry<String, com.google.gson.JsonElement> entry : tagObject.entrySet()) {
                if (entry.getValue().isJsonPrimitive()) {
                    tags.add(entry.getKey(), entry.getValue());
                }
            }
        }
        return this;
    }

    public MessageTags build() {
        return new MessageTags(tags);
    }
}
