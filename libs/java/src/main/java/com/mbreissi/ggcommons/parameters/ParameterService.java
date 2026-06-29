package com.mbreissi.ggcommons.parameters;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Optional;

import com.google.gson.Gson;
import com.google.gson.JsonElement;
import com.google.gson.JsonParseException;

/**
 * The public parameter interface (depend on this, not {@link DefaultParameterService}). Obtained
 * from the runtime via {@code getParameters()}. Reads are offline-first — served from the local
 * cache, never the network. Mirrors the Rust {@code ParameterService} trait (including its typed
 * accessors as Java {@code default} methods).
 */
public interface ParameterService {
    Gson GSON = new Gson();

    /** The value of {@code name} as a UTF-8 string, or empty. Served from the local cache (offline-first). */
    Optional<String> get(String name);

    /** The raw value bytes of {@code name}. */
    Optional<byte[]> getBytes(String name);

    /** All cached parameters under {@code path} (the prefix), as name -> string value. */
    Map<String, String> getByPath(String path);

    /** Cached parameter names under {@code prefix} (metadata only — no values). */
    List<String> names(String prefix);

    /** Force an immediate pull of the declared names/paths from the source into the cache. */
    void refresh();

    /** Non-sensitive stats for observability. */
    ParameterStats stats();

    /** The value parsed as an integer. */
    default Optional<Long> getInt(String name) {
        Optional<String> s = get(name);
        if (s.isEmpty()) {
            return Optional.empty();
        }
        try {
            return Optional.of(Long.parseLong(s.get().trim()));
        } catch (NumberFormatException e) {
            throw new ParameterException("parameter '" + name + "' is not an integer: " + e.getMessage());
        }
    }

    /** The value parsed as a boolean ({@code true}/{@code false}/{@code 1}/{@code 0}/…, case-insensitive). */
    default Optional<Boolean> getBool(String name) {
        Optional<String> s = get(name);
        if (s.isEmpty()) {
            return Optional.empty();
        }
        switch (s.get().trim().toLowerCase()) {
            case "true":
            case "1":
            case "yes":
            case "on":
                return Optional.of(true);
            case "false":
            case "0":
            case "no":
            case "off":
                return Optional.of(false);
            default:
                throw new ParameterException("parameter '" + name + "' is not a boolean: " + s.get().trim());
        }
    }

    /** The value parsed as JSON. */
    default Optional<JsonElement> getJson(String name) {
        Optional<byte[]> b = getBytes(name);
        if (b.isEmpty()) {
            return Optional.empty();
        }
        try {
            return Optional.of(GSON.fromJson(new String(b.get(), StandardCharsets.UTF_8), JsonElement.class));
        } catch (JsonParseException e) {
            throw new ParameterException("parameter '" + name + "' is not JSON: " + e.getMessage());
        }
    }

    /** A {@code StringList} value (comma-separated) as a list. */
    default Optional<List<String>> getStringList(String name) {
        Optional<String> s = get(name);
        if (s.isEmpty()) {
            return Optional.empty();
        }
        String v = s.get();
        List<String> out = new ArrayList<>();
        if (!v.isEmpty()) {
            for (String part : v.split(",", -1)) {
                out.add(part.trim());
            }
        }
        return Optional.of(out);
    }
}
