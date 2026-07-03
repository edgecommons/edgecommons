/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.messaging;

import com.mbreissi.ggcommons.config.ConfigManager;
import com.mbreissi.ggcommons.uns.Uns;
import com.mbreissi.ggcommons.uns.UnsClass;
import com.mbreissi.ggcommons.uns.UnsScope;
import com.mbreissi.ggcommons.uns.UnsValidationException;
import com.google.gson.Gson;
import com.google.gson.GsonBuilder;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;

import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;

/**
 * The shared execution engine for the cross-language {@code uns-test-vectors/} conformance suite
 * (UNS-CANONICAL-DESIGN §7 / D-U12/D-U13/D-U22): given a vector document, replays every case
 * through the REAL Java implementation ({@link Uns}, the {@link MessagingClient} reserved-class
 * guard predicate, {@link MessageBuilder}) and asserts the expected outputs — topics and filters
 * byte-for-byte, error codes exactly, envelopes structurally. Used by both the generator test
 * (self-check before writing / verify-in-place) and the loader test (conformance against the
 * committed files), so the two can never drift apart.
 *
 * <p>Vector contract mirrored by the Python/Rust/TS loaders (see {@code uns-test-vectors/README.md}):
 * <ul>
 *   <li><b>build</b> — {@code identityValues} and {@code component} pass through the template
 *       sanitizer ({@link ConfigManager#sanitize}) before identity construction (the config
 *       resolution path, D-U26 "sanitized &rArr; valid"); {@code instance} and {@code channel} are
 *       used VERBATIM (validated tokens, never sanitized). A missing {@code channel} key means
 *       "no channel".</li>
 *   <li><b>validate</b>/<b>filter</b> — the validator binds to a MULTI-level identity so the
 *       {@code includeRoot} input is the effective root mode (D-U25 makes includeRoot a no-op on
 *       single-level hierarchies).</li>
 *   <li><b>guard</b> — the §4.1 reserved-class predicate over {@code {topic, includeRoot}}.</li>
 *   <li><b>envelopes</b> — rebuilt via {@link MessageBuilder} with the pinned
 *       uuid/timestamp/correlation_id and the vector identity; compared structurally (member
 *       order is not normative, D-U22); the vector {@code topic} is also rebuilt byte-for-byte
 *       (includeRoot=false).</li>
 * </ul>
 */
final class UnsTestVectors {

    /** The shared vector directory at the repo root (test cwd is {@code libs/java}). */
    static final Path DIR = Path.of("..", "..", "uns-test-vectors");

    /** Serializer for the vector files: pretty, no HTML escaping (bodies carry {@code =} etc.). */
    static final Gson GSON = new GsonBuilder().setPrettyPrinting().disableHtmlEscaping().create();

    /**
     * The binding identity for validate/filter cases: MULTI-level so the case's
     * {@code includeRoot} input is effective (D-U25).
     */
    private static final MessageIdentity BINDING = new MessageIdentity(
            List.of(new MessageIdentity.HierEntry("site", "dallas"),
                    new MessageIdentity.HierEntry("device", "gw-01")),
            "opcua-adapter", "main");

    private UnsTestVectors() {
    }

    // ===================== document-level assertions (generator self-check + loader) ============

    /** Replays every build/validate/filter/guard case in a {@code topics.json} document. */
    static void assertTopicsDocument(JsonObject doc) {
        for (JsonElement caseEl : doc.getAsJsonArray("build")) {
            JsonObject c = caseEl.getAsJsonObject();
            assertEquals(c.getAsJsonObject("expected"), runBuild(c.getAsJsonObject("input")),
                    "build case '" + c.get("name").getAsString() + "'");
        }
        for (JsonElement caseEl : doc.getAsJsonArray("validate")) {
            JsonObject c = caseEl.getAsJsonObject();
            assertEquals(c.getAsJsonObject("expected"), runValidate(c.getAsJsonObject("input")),
                    "validate case '" + c.get("name").getAsString() + "'");
        }
        for (JsonElement caseEl : doc.getAsJsonArray("filter")) {
            JsonObject c = caseEl.getAsJsonObject();
            assertEquals(c.getAsJsonObject("expected"), runFilter(c.getAsJsonObject("input")),
                    "filter case '" + c.get("name").getAsString() + "'");
        }
        for (JsonElement caseEl : doc.getAsJsonArray("guard")) {
            JsonObject c = caseEl.getAsJsonObject();
            assertEquals(c.getAsJsonObject("expected"), runGuard(c.getAsJsonObject("input")),
                    "guard case '" + c.get("name").getAsString() + "'");
        }
    }

    /**
     * Replays every golden envelope in an {@code envelopes.json} document: the topic is rebuilt
     * byte-for-byte from the vector identity + class + channel, and the envelope is rebuilt via
     * {@link MessageBuilder} (pinned uuid/timestamp/correlation_id) and compared structurally.
     */
    static void assertEnvelopesDocument(JsonObject doc) {
        for (JsonElement caseEl : doc.getAsJsonArray("envelopes")) {
            JsonObject c = caseEl.getAsJsonObject();
            String name = c.get("name").getAsString();
            JsonObject envelope = c.getAsJsonObject("envelope");
            JsonObject header = envelope.getAsJsonObject("header");

            MessageIdentity identity = MessageIdentity.fromDict(envelope.getAsJsonObject("identity"));
            assertNotNull(identity, "envelope '" + name + "' identity must parse");

            // Topic reproduction, byte-for-byte (all envelope vectors are rootless).
            UnsClass cls = UnsClass.fromToken(c.get("class").getAsString());
            assertNotNull(cls, "envelope '" + name + "' class token");
            String channel = c.has("channel") ? c.get("channel").getAsString() : null;
            assertEquals(c.get("topic").getAsString(),
                    new Uns(identity, false).topic(cls, channel),
                    "envelope '" + name + "' topic");

            // Envelope reproduction through the single stamping site, compared STRUCTURALLY
            // (Gson JsonObject equality is member-order-insensitive - D-U22).
            Message rebuilt = MessageBuilder
                    .create(header.get("name").getAsString(), header.get("version").getAsString())
                    .withUuid(header.get("uuid").getAsString())
                    .withTimestamp(header.get("timestamp").getAsString())
                    .withCorrelationId(header.get("correlation_id").getAsString())
                    .withIdentity(identity)
                    .withPayload(envelope.get("body"))
                    .build();
            assertEquals(envelope, rebuilt.toDict(), "envelope '" + name + "'");
        }
    }

    // ===================== per-case runners (the language binding under test) ===================

    /**
     * Runs one build case: sanitize {@code identityValues}/{@code component} (the config
     * resolution path), construct the identity, build the topic. Returns {@code {"topic": …}} or
     * {@code {"error": "<CODE>"}}.
     */
    static JsonObject runBuild(JsonObject input) {
        JsonObject values = input.getAsJsonObject("identityValues");
        List<MessageIdentity.HierEntry> hier = new ArrayList<>();
        for (JsonElement levelEl : input.getAsJsonArray("hierarchyLevels")) {
            String level = levelEl.getAsString();
            hier.add(new MessageIdentity.HierEntry(level,
                    ConfigManager.sanitize(values.get(level).getAsString())));
        }
        MessageIdentity identity = new MessageIdentity(hier,
                ConfigManager.sanitize(input.get("component").getAsString()),
                input.get("instance").getAsString());
        UnsClass cls = UnsClass.fromToken(input.get("class").getAsString());
        assertNotNull(cls, "build input class token '" + input.get("class").getAsString() + "'");
        String channel = input.has("channel") ? input.get("channel").getAsString() : null;
        try {
            String topic = new Uns(identity, input.get("includeRoot").getAsBoolean())
                    .topic(cls, channel);
            JsonObject out = new JsonObject();
            out.addProperty("topic", topic);
            return out;
        } catch (UnsValidationException e) {
            return error(e);
        }
    }

    /** Runs one validate case (bound to the multi-level {@link #BINDING} identity). */
    static JsonObject runValidate(JsonObject input) {
        try {
            new Uns(BINDING, input.get("includeRoot").getAsBoolean())
                    .validate(input.get("topic").getAsString());
            JsonObject out = new JsonObject();
            out.addProperty("ok", true);
            return out;
        } catch (UnsValidationException e) {
            return error(e);
        }
    }

    /** Runs one filter case (absent scope fields are {@code null} &rarr; {@code +}). */
    static JsonObject runFilter(JsonObject input) {
        JsonObject scope = input.getAsJsonObject("scope");
        UnsScope unsScope = new UnsScope(
                optional(scope, "site"), optional(scope, "device"),
                optional(scope, "component"), optional(scope, "instance"));
        UnsClass cls = UnsClass.fromToken(input.get("class").getAsString());
        assertNotNull(cls, "filter input class token '" + input.get("class").getAsString() + "'");
        String filter = new Uns(BINDING, input.get("includeRoot").getAsBoolean())
                .filter(cls, unsScope);
        JsonObject out = new JsonObject();
        out.addProperty("filter", filter);
        return out;
    }

    /** Runs one guard case through the §4.1 reserved-class predicate (slice 1d / D-U24). */
    static JsonObject runGuard(JsonObject input) {
        UnsClass reserved = MessagingClient.reservedClassOf(
                input.get("topic").getAsString(), input.get("includeRoot").getAsBoolean());
        JsonObject out = new JsonObject();
        out.addProperty("reserved", reserved != null);
        return out;
    }

    private static JsonObject error(UnsValidationException e) {
        JsonObject out = new JsonObject();
        out.addProperty("error", e.getCode().name());
        return out;
    }

    private static String optional(JsonObject obj, String key) {
        return obj != null && obj.has(key) ? obj.get(key).getAsString() : null;
    }
}
