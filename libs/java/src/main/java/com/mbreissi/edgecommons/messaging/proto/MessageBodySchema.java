package com.mbreissi.edgecommons.messaging.proto;

import com.google.gson.JsonObject;

public record MessageBodySchema(String name, String version, String contentType,
                                String descriptorRef, String hash) {

    public JsonObject toDict() {
        JsonObject obj = new JsonObject();
        if (name != null) {
            obj.addProperty("name", name);
        }
        if (version != null) {
            obj.addProperty("version", version);
        }
        if (contentType != null) {
            obj.addProperty("content_type", contentType);
        }
        if (descriptorRef != null) {
            obj.addProperty("descriptor_ref", descriptorRef);
        }
        if (hash != null) {
            obj.addProperty("hash", hash);
        }
        return obj;
    }

    public static MessageBodySchema fromDict(JsonObject src) {
        if (src == null) {
            return null;
        }
        return new MessageBodySchema(
                stringOrNull(src, "name"),
                stringOrNull(src, "version"),
                stringOrNull(src, "content_type"),
                stringOrNull(src, "descriptor_ref"),
                stringOrNull(src, "hash"));
    }

    private static String stringOrNull(JsonObject src, String key) {
        return src.has(key) && src.get(key).isJsonPrimitive() ? src.get(key).getAsString() : null;
    }
}
