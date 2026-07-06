package com.mbreissi.edgecommons.credentials;

import java.util.Map;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;

/**
 * Resolve {@code $secret} references in any config element against the vault — mirrors the Rust
 * {@code resolve_secret_refs}. A {@code {"$secret": "name"}} object is replaced by the secret's whole
 * value (as a string); {@code {"$secret": "name", "field": "k"}} by field {@code k} of the secret's
 * JSON. Resolved lazily at subsystem-init time so the secret never lands in the logged/templated
 * config snapshot.
 *
 * <p>This is how streaming/messaging consume credentials (closes {@code TELEMETRY_STREAMING.md} §7).
 * The shared config is never mutated — resolution operates on a deep copy. Secret values are never
 * logged.
 */
public final class SecretRefs {
    private SecretRefs() {}

    /**
     * Return a deep copy of {@code element} with every {@code $secret} reference replaced by its
     * resolved value from {@code creds}. The input element is never mutated.
     *
     * @throws CredentialException if a referenced secret (or requested field) is absent.
     */
    public static JsonElement resolve(JsonElement element, CredentialService creds) {
        if (element == null || element.isJsonNull()) {
            return element;
        }
        JsonElement copy = element.deepCopy();
        return resolveInPlace(copy, creds);
    }

    private static JsonElement resolveInPlace(JsonElement value, CredentialService creds) {
        if (value.isJsonObject()) {
            JsonObject obj = value.getAsJsonObject();
            if (obj.has("$secret") && obj.get("$secret").isJsonPrimitive()) {
                String name = obj.get("$secret").getAsString();
                String field = obj.has("field") && obj.get("field").isJsonPrimitive()
                        ? obj.get("field").getAsString() : null;
                return new com.google.gson.JsonPrimitive(resolveOne(name, field, creds));
            }
            for (Map.Entry<String, JsonElement> e : obj.entrySet()) {
                e.setValue(resolveInPlace(e.getValue(), creds));
            }
            return obj;
        }
        if (value.isJsonArray()) {
            JsonArray arr = value.getAsJsonArray();
            for (int i = 0; i < arr.size(); i++) {
                arr.set(i, resolveInPlace(arr.get(i), creds));
            }
            return arr;
        }
        return value;
    }

    private static String resolveOne(String name, String field, CredentialService creds) {
        if (field == null) {
            return creds.getString(name)
                    .orElseThrow(() -> new CredentialException("secretRef '" + name + "' not found in the vault"));
        }
        JsonElement json = creds.getJson(name)
                .orElseThrow(() -> new CredentialException("secretRef '" + name + "' not found in the vault"));
        if (!json.isJsonObject()) {
            throw new CredentialException("secretRef '" + name + "' field '" + field + "' missing or not a string");
        }
        JsonObject obj = json.getAsJsonObject();
        if (!obj.has(field) || !obj.get(field).isJsonPrimitive() || !obj.getAsJsonPrimitive(field).isString()) {
            throw new CredentialException("secretRef '" + name + "' field '" + field + "' missing or not a string");
        }
        return obj.get(field).getAsString();
    }
}
