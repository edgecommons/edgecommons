/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.ArrayList;
import java.util.List;
import java.util.stream.Collectors;

/**
 * The top-level {@code identity} envelope element of the unified namespace (UNS).
 *
 * <p>One immutable class serves as both the wire object and the component's resolved identity
 * (see {@code ConfigManager.getComponentIdentity()}). It carries:
 * <ul>
 *   <li>{@code hier} — the ordered enterprise hierarchy (size &gt;= 1); its <b>last entry is
 *       always the physical device</b>. There is no standalone {@code device} wire field —
 *       {@link #getDevice()} is a computed accessor over the last entry.</li>
 *   <li>{@code path} — the precomputed {@code '/'}-join of the {@code hier} values. The publisher
 *       is authoritative: on deserialize a present {@code path} is taken as-is, a missing one is
 *       recomputed.</li>
 *   <li>{@code component} — the publishing component's UNS token (the sanitized short name,
 *       i.e. the existing {@code {ComponentName}} semantics).</li>
 *   <li>{@code instance} — the per-message instance token, or {@code null} for
 *       component/global scope (D‑U28); never a reserved class token.</li>
 * </ul>
 *
 * <p>Serialization ({@link #toDict()}) emits the canonical member order
 * {@code hier, path, component, instance}. Deserialization ({@link #fromDict}) is deliberately
 * lenient, mirroring the lenient envelope handling across all four libraries: a malformed
 * {@code identity} yields {@code null} plus a WARN log, and the message still delivers.
 */
public final class MessageIdentity {

    private static final Logger LOGGER = LogManager.getLogger(MessageIdentity.class);

    /**
     * Class tokens an instance id may not equal (D‑U28): keeping the component-scope and
     * instance-scope UNS subscription templates disjoint, and letting the reserved-class guard
     * locate the class unambiguously.
     */
    private static final java.util.Set<String> RESERVED_CLASS_TOKENS =
            java.util.Set.of("state", "metric", "cfg", "log", "data", "evt", "cmd", "app");

    /**
     * One level of the enterprise hierarchy: the level's configured {@code level} name and this
     * deployment's {@code value} for it. Both parts must be non-null and non-empty.
     */
    public record HierEntry(String level, String value) {
        public HierEntry {
            if (level == null || level.isEmpty()) {
                throw new IllegalArgumentException("MessageIdentity hier entry level must be non-empty");
            }
            if (value == null || value.isEmpty()) {
                throw new IllegalArgumentException(
                        "MessageIdentity hier entry value for level '" + level + "' must be non-empty");
            }
        }
    }

    private final List<HierEntry> hier;   // immutable, size >= 1; last = device
    private final String path;            // precomputed '/'-join of the hier values
    private final String component;       // UNS component token (sanitized short name)
    private final String instance;        // null ⇒ component scope (D‑U28); never a class token

    /**
     * Creates a validated identity, precomputing {@code path} as the {@code '/'}-join of the
     * {@code hier} values.
     *
     * @param hier      the ordered hierarchy entries (non-null, size &gt;= 1; last entry = device)
     * @param component the component UNS token (non-null, non-empty)
     * @param instance  the instance token, or {@code null}/empty for component/global scope (D‑U28)
     * @throws IllegalArgumentException if {@code hier} is null/empty or {@code component} is
     *                                  null/empty (entry-level validation is in {@link HierEntry})
     */
    public MessageIdentity(List<HierEntry> hier, String component, String instance) {
        this(hier, null, component, instance);
    }

    /**
     * Internal constructor allowing an explicit {@code path} (used by {@link #fromDict} where a
     * present wire path is authoritative). A {@code null} path is recomputed from {@code hier}.
     */
    private MessageIdentity(List<HierEntry> hier, String path, String component, String instance) {
        if (hier == null || hier.isEmpty()) {
            throw new IllegalArgumentException("MessageIdentity hier must contain at least one entry");
        }
        if (component == null || component.isEmpty()) {
            throw new IllegalArgumentException("MessageIdentity component must be non-empty");
        }
        this.hier = List.copyOf(hier);
        this.path = (path != null)
                ? path
                : this.hier.stream().map(HierEntry::value).collect(Collectors.joining("/"));
        this.component = component;
        // D‑U28: absent/empty ⇒ component scope (null); a present instance may not be a class token.
        if (instance != null && !instance.isEmpty() && RESERVED_CLASS_TOKENS.contains(instance)) {
            throw new IllegalArgumentException(
                    "MessageIdentity instance '" + instance + "' must not be a reserved UNS class token");
        }
        this.instance = (instance == null || instance.isEmpty()) ? null : instance;
    }

    /** Returns the immutable, ordered hierarchy entries (the last entry is the device). */
    public List<HierEntry> getHier() {
        return hier;
    }

    /** Returns the precomputed {@code '/'}-join of the hierarchy values. */
    public String getPath() {
        return path;
    }

    /** Returns the component UNS token (the sanitized short name). */
    public String getComponent() {
        return component;
    }

    /** Returns the per-message instance token, or {@code null} for component/global scope (D‑U28). */
    public String getInstance() {
        return instance;
    }

    /**
     * Computed accessor — the last {@code hier} entry's value. NOT a wire field: the device is
     * inherent to the hierarchy (its deepest level), so it is never serialized separately.
     */
    public String getDevice() {
        return hier.get(hier.size() - 1).value();
    }

    /**
     * Returns a copy of this identity with a different per-message instance token, or component
     * scope when {@code instance} is {@code null}/empty (D‑U28).
     *
     * @param instance the instance token, or {@code null}/empty for component/global scope
     * @throws IllegalArgumentException if {@code instance} is a reserved class token
     */
    public MessageIdentity withInstance(String instance) {
        return new MessageIdentity(hier, path, component, instance);
    }

    /**
     * Serializes this identity to its wire form, in the canonical member order
     * {@code hier, path, component, instance}.
     *
     * @return the identity as a {@link JsonObject}
     */
    public JsonObject toDict() {
        JsonObject retVal = new JsonObject();
        JsonArray hierArray = new JsonArray();
        for (HierEntry entry : hier) {
            JsonObject entryObj = new JsonObject();
            entryObj.addProperty("level", entry.level());
            entryObj.addProperty("value", entry.value());
            hierArray.add(entryObj);
        }
        retVal.add("hier", hierArray);
        retVal.addProperty("path", path);
        retVal.addProperty("component", component);
        if (instance != null) {
            retVal.addProperty("instance", instance);   // D‑U28: omitted when component-scoped
        }
        return retVal;
    }

    /**
     * Lenient wire-form parser: a missing {@code instance} means component scope (absent, D‑U28);
     * a missing {@code path} is recomputed from the hier values (a present one is taken as-is —
     * the publisher is authoritative); a malformed identity (missing/empty/non-array {@code hier},
     * malformed hier entries, or a missing {@code component}) yields {@code null} plus a WARN log
     * so the enclosing message still delivers.
     *
     * @param src the wire-form identity object (may be {@code null})
     * @return the parsed identity, or {@code null} when malformed
     */
    public static MessageIdentity fromDict(JsonObject src) {
        if (src == null) {
            LOGGER.warn("Malformed message identity: null identity object; dropping identity");
            return null;
        }
        try {
            JsonElement hierEl = src.get("hier");
            if (hierEl == null || !hierEl.isJsonArray() || hierEl.getAsJsonArray().isEmpty()) {
                LOGGER.warn("Malformed message identity: 'hier' missing, not an array, or empty; dropping identity");
                return null;
            }
            List<HierEntry> hier = new ArrayList<>();
            for (JsonElement entryEl : hierEl.getAsJsonArray()) {
                if (!entryEl.isJsonObject()) {
                    LOGGER.warn("Malformed message identity: hier entry is not an object; dropping identity");
                    return null;
                }
                JsonObject entryObj = entryEl.getAsJsonObject();
                String level = asNonEmptyString(entryObj.get("level"));
                String value = asNonEmptyString(entryObj.get("value"));
                if (level == null || value == null) {
                    LOGGER.warn("Malformed message identity: hier entry missing level/value; dropping identity");
                    return null;
                }
                hier.add(new HierEntry(level, value));
            }
            String component = asNonEmptyString(src.get("component"));
            if (component == null) {
                LOGGER.warn("Malformed message identity: 'component' missing or empty; dropping identity");
                return null;
            }
            String path = asNonEmptyString(src.get("path"));         // null -> recomputed
            String instance = asNonEmptyString(src.get("instance")); // null -> component scope (D‑U28)
            return new MessageIdentity(hier, path, component, instance);
        } catch (RuntimeException e) {
            LOGGER.warn("Malformed message identity ({}); dropping identity", e.getMessage());
            return null;
        }
    }

    /** Returns the element as a non-empty string, or {@code null} if absent/non-string/empty. */
    private static String asNonEmptyString(JsonElement element) {
        if (element == null || !element.isJsonPrimitive() || !element.getAsJsonPrimitive().isString()) {
            return null;
        }
        String value = element.getAsString();
        return value.isEmpty() ? null : value;
    }

    @Override
    public String toString() {
        return toDict().toString();
    }
}
