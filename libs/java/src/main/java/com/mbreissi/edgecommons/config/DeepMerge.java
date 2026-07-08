package com.mbreissi.edgecommons.config;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.List;

/**
 * Split-config deep merge. Later layers win; objects merge recursively; arrays, scalars and null
 * replace. Raw-layer control fields are stripped from every layer before merging.
 */
final class DeepMerge {
    private static final Logger LOGGER = LogManager.getLogger(DeepMerge.class);
    static final String EXTENDS = "extends";
    static final String SHARED_CONFIG = "sharedConfig";

    private DeepMerge() {
    }

    static JsonObject merge(List<JsonObject> layers) {
        JsonObject result = new JsonObject();
        for (JsonObject layer : layers) {
            if (layer == null) {
                continue;
            }
            mergeObject(result, stripControls(layer), "$");
        }
        return result;
    }

    static JsonObject stripControls(JsonObject layer) {
        JsonObject stripped = layer.deepCopy();
        stripped.remove(EXTENDS);
        stripped.remove(SHARED_CONFIG);
        return stripped;
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
                    LOGGER.warn("split-config type conflict at {}; component layer wins", childPath);
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
