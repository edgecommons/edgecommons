package com.mbreissi.edgecommons.config;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.List;

/**
 * Hierarchical-config deep merge. Later layers win; objects merge recursively; arrays, scalars
 * and null replace.
 */
final class DeepMerge {
    private static final Logger LOGGER = LogManager.getLogger(DeepMerge.class);

    private DeepMerge() {
    }

    static JsonObject merge(List<JsonObject> layers) {
        JsonObject result = new JsonObject();
        for (JsonObject layer : layers) {
            if (layer == null) {
                continue;
            }
            mergeObject(result, layer, "$");
        }
        return result;
    }

    private static void mergeObject(JsonObject target, JsonObject incoming, String path) {
        for (String key : incoming.keySet()) {
            JsonElement next = incoming.get(key);
            JsonElement current = target.get(key);
            String childPath = "$".equals(path) ? "$." + key : path + "." + key;
            if (current != null && current.isJsonObject() && next != null && next.isJsonObject()) {
                mergeObject(current.getAsJsonObject(), next.getAsJsonObject(), childPath);
            } else {
                if (current != null && next != null && incompatibleForWarning(current, next)) {
                    LOGGER.warn("hierarchical-config type conflict at {}; later layer wins", childPath);
                }
                target.add(key, next == null ? null : next.deepCopy());
            }
        }
    }

    private static boolean incompatibleForWarning(JsonElement current, JsonElement next) {
        if (current.isJsonArray() || next.isJsonArray()) {
            return false;
        }
        if (current.isJsonNull() || next.isJsonNull()) {
            return false;
        }
        return kind(current) != kind(next);
    }

    private static int kind(JsonElement element) {
        if (element == null || element.isJsonNull()) {
            return 0;
        }
        if (element.isJsonObject()) {
            return 1;
        }
        if (element.isJsonArray()) {
            return 2;
        }
        return 3;
    }
}
