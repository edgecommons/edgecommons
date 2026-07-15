/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import com.mbreissi.edgecommons.commands.CommandInbox;
import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.facades.AppFacade;
import com.mbreissi.edgecommons.facades.Channel;
import com.mbreissi.edgecommons.facades.DataFacade;
import com.mbreissi.edgecommons.facades.EventsFacade;
import com.mbreissi.edgecommons.facades.Quality;
import com.mbreissi.edgecommons.facades.Severity;
import com.mbreissi.edgecommons.facades.SignalUpdate;
import com.mbreissi.edgecommons.facades.StreamSink;
import com.mbreissi.edgecommons.test.MockConfigurationService;
import com.mbreissi.edgecommons.test.MockMessagingService;
import com.mbreissi.edgecommons.uns.RepublishListener;
import com.mbreissi.edgecommons.uns.Uns;
import com.mbreissi.edgecommons.uns.UnsClass;
import com.mbreissi.edgecommons.uns.UnsScope;
import com.mbreissi.edgecommons.uns.UnsValidationException;
import com.google.gson.Gson;
import com.google.gson.GsonBuilder;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;

import java.nio.file.Path;
import java.time.Clock;
import java.time.Instant;
import java.time.ZoneOffset;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

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

    /**
     * The pinned clock for the publish-facade vectors (data/evt): the {@code serverTs}/
     * {@code timestamp} time defaults resolve to {@value #FACADE_NOW} deterministically — the same
     * injected-clock discipline the {@code _bcast} listener uses.
     */
    static final String FACADE_NOW = "2026-07-01T12:00:00Z";
    private static final Clock FIXED_CLOCK =
            Clock.fixed(Instant.parse(FACADE_NOW), ZoneOffset.UTC);

    /** The single-level identity the publish-facade vectors bind to (device {@code gw-01}). */
    private static final MessageIdentity FACADE_IDENTITY = new MessageIdentity(
            List.of(new MessageIdentity.HierEntry("device", "gw-01")), "opcua-adapter", "main");

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

    /**
     * Replays a {@code bcast.json} document — the {@code _bcast} republish (reconnect-rehydration)
     * contract (DESIGN-uns §9.3/§9.4): the two broadcast command topics are rebuilt byte-for-byte
     * through the topic builder with the {@code _bcast} pseudo-component identity; the golden
     * notification envelopes ({@code {header, body:{}}} — no identity, no tags, no
     * {@code reply_to}) are rebuilt via {@link MessageBuilder} and compared structurally; and the
     * normative listener behavior constants (jitter window, cooldown) must equal the live
     * {@link RepublishListener} implementation's.
     */
    static void assertBcastDocument(JsonObject doc) {
        String device = doc.get("device").getAsString();
        JsonElement[] cases = doc.getAsJsonArray("commands").asList().toArray(new JsonElement[0]);
        assertEquals(2, cases.length, "bcast.json must pin exactly the two republish commands");
        String[] expectedVerbs = {RepublishListener.REPUBLISH_STATE, RepublishListener.REPUBLISH_CFG};
        for (int i = 0; i < cases.length; i++) {
            JsonObject c = cases[i].getAsJsonObject();
            String name = c.get("name").getAsString();
            assertEquals(expectedVerbs[i], name, "bcast command #" + i + " verb");

            // Topic reproduction, byte-for-byte, through the real builder with the reserved
            // _bcast pseudo-component identity (single-level -> rootless by D-U25).
            JsonObject input = c.getAsJsonObject("input");
            assertEquals(device, input.get("device").getAsString(), "'" + name + "' input device");
            assertEquals(RepublishListener.BCAST_COMPONENT, input.get("component").getAsString(),
                    "'" + name + "' pseudo-component token");
            MessageIdentity bcast = new MessageIdentity(
                    List.of(new MessageIdentity.HierEntry("device", input.get("device").getAsString())),
                    input.get("component").getAsString(), input.get("instance").getAsString());
            String topic = new Uns(bcast, input.get("includeRoot").getAsBoolean())
                    .topic(UnsClass.fromToken(input.get("class").getAsString()),
                            input.get("channel").getAsString());
            assertEquals(c.get("topic").getAsString(), topic, "'" + name + "' topic");

            // Envelope reproduction: a notification-style cmd envelope - no identity, no tags,
            // no reply_to, empty body; header.name = the verb, version pinned to 1.0.
            JsonObject envelope = c.getAsJsonObject("envelope");
            JsonObject header = envelope.getAsJsonObject("header");
            assertEquals(name, header.get("name").getAsString(), "'" + name + "' header name");
            assertEquals("1.0", header.get("version").getAsString(), "'" + name + "' header version");
            assertEquals(false, header.has("reply_to"),
                    "'" + name + "' is fire-and-forget - no reply_to");
            assertEquals(false, envelope.has("identity"),
                    "'" + name + "' is built without a config-bound builder - no identity");
            assertEquals(false, envelope.has("tags"), "'" + name + "' carries no tags");
            assertEquals(new JsonObject(), envelope.getAsJsonObject("body"),
                    "'" + name + "' body is the empty object");
            Message rebuilt = MessageBuilder
                    .create(header.get("name").getAsString(), header.get("version").getAsString())
                    .withUuid(header.get("uuid").getAsString())
                    .withTimestamp(header.get("timestamp").getAsString())
                    .withCorrelationId(header.get("correlation_id").getAsString())
                    .withPayload(envelope.get("body"))
                    .build();
            assertEquals(envelope, rebuilt.toDict(), "'" + name + "' envelope");
        }

        // The normative listener behavior constants (all four languages).
        JsonObject behavior = doc.getAsJsonObject("behavior");
        assertEquals(RepublishListener.JITTER_WINDOW_MS, behavior.get("jitterWindowMs").getAsLong(),
                "jitterWindowMs must equal the implementation's window");
        assertEquals(RepublishListener.COOLDOWN_MS, behavior.get("cooldownMs").getAsLong(),
                "cooldownMs must equal the implementation's cooldown");
        assertEquals(false, behavior.get("replyTo").getAsBoolean(),
                "the republish broadcast never carries a reply_to");
    }

    /**
     * Replays a {@code commands.json} document — the command-inbox contract (DESIGN-uns §9.5,
     * the minimal {@code commands()} facade, edge-console slice S2): the own-inbox wildcard is
     * rebuilt byte-for-byte through the topic builder; each verb's request topic and golden
     * request/reply envelopes are rebuilt through {@link Uns}/{@link MessageBuilder} and compared
     * structurally (the reply must carry the request's {@code correlation_id} and no
     * {@code reply_to}); every request is then <b>dispatched through a LIVE {@link CommandInbox}</b>
     * (pinned action seams: uptime 42 s, reload success, the vector's own config snapshot) whose
     * published reply body must equal the golden body byte-for-byte; and the behavior flags/sets
     * must equal the implementation's pinned constants.
     */
    static void assertCommandsDocument(JsonObject doc) {
        // ---- the inbox subscription, byte-for-byte through the real builder ----
        JsonObject inbox = doc.getAsJsonObject("inbox");
        JsonObject input = inbox.getAsJsonObject("input");
        MessageIdentity identity = new MessageIdentity(
                List.of(new MessageIdentity.HierEntry("device", input.get("device").getAsString())),
                input.get("component").getAsString(), input.get("instance").getAsString());
        Uns inboxUns = new Uns(identity, input.get("includeRoot").getAsBoolean());
        UnsScope inboxScope = new UnsScope(null, identity.getDevice(), identity.getComponent(),
                identity.getInstance());
        UnsClass inboxClass = UnsClass.fromToken(input.get("class").getAsString());
        // D‑U28: the inbox subscribes both the instance-scope and the component-scope cmd filters.
        String filter = inboxUns.filter(inboxClass, inboxScope);
        String componentFilter = inboxUns.filter(inboxClass, inboxScope, false);
        assertEquals(inbox.get("filter").getAsString(), filter, "inbox filter");
        assertEquals(inbox.get("componentFilter").getAsString(), componentFilter,
                "inbox component-scope filter");

        // ---- a LIVE inbox with the pinned action seams the vectors document ----
        JsonArray verbs = doc.getAsJsonArray("verbs");
        assertEquals(5, verbs.size(), "commands.json must pin exactly the five built-in verbs");
        String[] expectedVerbs = {CommandInbox.PING, CommandInbox.DESCRIBE, CommandInbox.RELOAD_CONFIG,
                CommandInbox.GET_CONFIGURATION, CommandInbox.STATUS};
        JsonObject pinnedConfig = findCommandCase(verbs, CommandInbox.GET_CONFIGURATION)
                .getAsJsonObject("reply").getAsJsonObject("body")
                .getAsJsonObject("result").getAsJsonObject("config");
        MockConfigurationService config = new MockConfigurationService();
        config.setComponentIdentity(identity);
        MockMessagingService messaging = new MockMessagingService();
        CommandInbox live = new CommandInbox(config, messaging,
                () -> 42L, () -> true, pinnedConfig::deepCopy);
        live.start();
        assertEquals(Set.of(filter, componentFilter), messaging.getSubscribedTopics(),
                "the live inbox must subscribe exactly the pinned instance- and component-scope"
                        + " filters");

        for (int i = 0; i < verbs.size(); i++) {
            JsonObject c = verbs.get(i).getAsJsonObject();
            assertEquals(expectedVerbs[i], c.get("name").getAsString(), "verb #" + i);
            assertCommandCase(c, identity, messaging, true);
        }
        for (JsonElement caseEl : doc.getAsJsonArray("errors")) {
            assertCommandCase(caseEl.getAsJsonObject(), identity, messaging, false);
        }
        live.close();

        // ---- the normative behavior flags/sets (all four languages) ----
        JsonObject behavior = doc.getAsJsonObject("behavior");
        assertEquals(true, behavior.get("verbIsTopicChannel").getAsBoolean(),
                "the verb is the cmd topic's channel");
        assertEquals(true, behavior.get("headerNameMustEqualVerb").getAsBoolean(),
                "header.name must equal the topic verb");
        assertEquals(true, behavior.get("fireAndForgetWithoutReplyTo").getAsBoolean(),
                "a cmd without reply_to is fire-and-forget");
        assertEquals(true, behavior.get("malformedIgnoredWithoutReply").getAsBoolean(),
                "malformed/foreign payloads are ignored, never replied to");
        assertEquals(CommandInbox.BUILT_IN_VERBS, stringSet(behavior, "builtInVerbs"),
                "builtInVerbs must equal the implementation's set");
        assertEquals(CommandInbox.DELEGATED_VERBS, stringSet(behavior, "delegatedVerbs"),
                "delegatedVerbs must equal the implementation's set");
        assertEquals(Set.of(CommandInbox.ERR_UNKNOWN_VERB, CommandInbox.ERR_HANDLER_ERROR,
                        CommandInbox.ERR_RELOAD_FAILED, CommandInbox.ERR_NO_CONFIG),
                stringSet(behavior, "errorCodes"),
                "errorCodes must equal the implementation's pinned base codes");
    }

    private static JsonObject findCommandCase(JsonArray verbs, String name) {
        for (JsonElement el : verbs) {
            JsonObject c = el.getAsJsonObject();
            if (name.equals(c.get("name").getAsString())) {
                return c;
            }
        }
        throw new AssertionError("commands.json missing command case " + name);
    }

    /**
     * One command vector: the topic byte-for-byte, the request/reply envelopes structurally
     * (rebuilt via {@link MessageBuilder}; the request's {@code reply_to} via the real
     * {@code makeRequest} path), the correlation rule, and the LIVE round trip — the request is
     * replayed through the subscribed inbox and the published reply (topic = the request's
     * {@code reply_to}) must carry the golden body.
     */
    private static void assertCommandCase(JsonObject c, MessageIdentity identity,
            MockMessagingService messaging, boolean expectOk) {
        String name = c.get("name").getAsString();
        String verb = c.get("verb").getAsString();

        // Topic reproduction, byte-for-byte (the verb is the cmd channel).
        assertEquals(c.get("topic").getAsString(),
                new Uns(identity, false).topic(UnsClass.CMD, verb), "'" + name + "' topic");

        // Request envelope reproduction: pinned header fields + the real makeRequest path for
        // reply_to; no identity (the requester's identity is not part of the dispatch contract).
        JsonObject request = c.getAsJsonObject("request");
        JsonObject requestHeader = request.getAsJsonObject("header");
        assertEquals(verb, requestHeader.get("name").getAsString(),
                "'" + name + "' request header.name must equal the verb");
        Message rebuiltRequest = MessageBuilder
                .create(requestHeader.get("name").getAsString(),
                        requestHeader.get("version").getAsString())
                .withUuid(requestHeader.get("uuid").getAsString())
                .withTimestamp(requestHeader.get("timestamp").getAsString())
                .withCorrelationId(requestHeader.get("correlation_id").getAsString())
                .withPayload(request.get("body"))
                .build();
        rebuiltRequest.makeRequest(requestHeader.get("reply_to").getAsString());
        assertEquals(request, rebuiltRequest.toDict(), "'" + name + "' request envelope");

        // Reply envelope reproduction: the responder's identity, the REQUEST's correlation_id,
        // never a reply_to.
        JsonObject reply = c.getAsJsonObject("reply");
        JsonObject replyHeader = reply.getAsJsonObject("header");
        assertEquals(verb, replyHeader.get("name").getAsString(),
                "'" + name + "' reply header.name must equal the verb");
        assertEquals(CommandInbox.CMD_MESSAGE_VERSION, replyHeader.get("version").getAsString(),
                "'" + name + "' reply header.version");
        assertEquals(requestHeader.get("correlation_id").getAsString(),
                replyHeader.get("correlation_id").getAsString(),
                "'" + name + "' reply must carry the request's correlation_id");
        assertEquals(false, replyHeader.has("reply_to"), "'" + name + "' reply has no reply_to");
        MessageIdentity replyIdentity = MessageIdentity.fromDict(reply.getAsJsonObject("identity"));
        assertNotNull(replyIdentity, "'" + name + "' reply identity must parse");
        Message rebuiltReply = MessageBuilder
                .create(replyHeader.get("name").getAsString(),
                        replyHeader.get("version").getAsString())
                .withUuid(replyHeader.get("uuid").getAsString())
                .withTimestamp(replyHeader.get("timestamp").getAsString())
                .withCorrelationId(replyHeader.get("correlation_id").getAsString())
                .withIdentity(replyIdentity)
                .withPayload(reply.get("body"))
                .build();
        assertEquals(reply, rebuiltReply.toDict(), "'" + name + "' reply envelope");

        // Reply body shape: {ok:true, result:{…}} or {ok:false, error:{code, message}}.
        JsonObject body = reply.getAsJsonObject("body");
        assertEquals(expectOk, body.get("ok").getAsBoolean(), "'" + name + "' reply ok flag");
        assertEquals(true, body.has(expectOk ? "result" : "error"),
                "'" + name + "' reply body carries " + (expectOk ? "result" : "error"));

        // LIVE round trip: the real inbox must publish exactly the golden reply body to the
        // request's reply_to.
        messaging.clearPublishedMessages();
        messaging.simulateMessage(c.get("topic").getAsString(),
                MessageBuilder.fromObject(request.deepCopy()));
        assertEquals(1, messaging.getPublishedMessages().size(),
                "'" + name + "' live dispatch must publish exactly one reply");
        MockMessagingService.PublishedMessage published = messaging.getPublishedMessages().get(0);
        assertEquals(requestHeader.get("reply_to").getAsString(), published.topic,
                "'" + name + "' live reply must go to the request's reply_to");
        assertEquals(body, published.message.toDict().get("body"),
                "'" + name + "' live reply body must equal the golden body");
    }

    /** Reads a behavior string array as a set. */
    private static Set<String> stringSet(JsonObject behavior, String key) {
        Set<String> values = new HashSet<>();
        for (JsonElement el : behavior.getAsJsonArray(key)) {
            values.add(el.getAsString());
        }
        return values;
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

    // ===================== data/evt/app publish-facade documents ================================

    /**
     * Replays every {@code data.json} case through a LIVE {@link DataFacade} (fixed clock, mock
     * messaging + recording stream sink) and asserts the pinned {@code {topic, route, body[,
     * partitionKey]}} — or {@code {throws:true}} — output. This binds the mirrors to the body
     * defaulting rules (quality → {@code GOOD} + {@code qualityRaw:"unspecified"}, {@code serverTs}
     * → now, samples wrapper), the channel sanitization, and the per-call channel routing.
     */
    static void assertDataDocument(JsonObject doc) {
        for (JsonElement caseEl : doc.getAsJsonArray("cases")) {
            JsonObject c = caseEl.getAsJsonObject();
            assertEquals(c.getAsJsonObject("expected"), runDataCase(c.getAsJsonObject("input")),
                    "data case '" + c.get("name").getAsString() + "'");
        }
    }

    /**
     * Replays every {@code evt.json} case through a LIVE {@link EventsFacade}: pins the
     * {@code evt/{severity}/{type}} channel DERIVED from the body, the four severity tokens, the
     * {@code timestamp} → now default, and the {@code raiseAlarm}/{@code clearAlarm}
     * {@code alarm}/{@code active} fields.
     */
    static void assertEvtDocument(JsonObject doc) {
        for (JsonElement caseEl : doc.getAsJsonArray("cases")) {
            JsonObject c = caseEl.getAsJsonObject();
            assertEquals(c.getAsJsonObject("expected"), runEvtCase(c.getAsJsonObject("input")),
                    "evt case '" + c.get("name").getAsString() + "'");
        }
    }

    /**
     * Replays every {@code app.json} case through a LIVE {@link AppFacade}: pins the thin-facade
     * guarantee (body passed through verbatim, header {@code name} = the caller's name, topic =
     * {@code app/{channel}}).
     */
    static void assertAppDocument(JsonObject doc) {
        for (JsonElement caseEl : doc.getAsJsonArray("cases")) {
            JsonObject c = caseEl.getAsJsonObject();
            assertEquals(c.getAsJsonObject("expected"), runAppCase(c.getAsJsonObject("input")),
                    "app case '" + c.get("name").getAsString() + "'");
        }
    }

    /** A recording {@link StreamSink}: captures the last (and counts all) stream appends. */
    private static final class RecordingStreamSink implements StreamSink {
        private String streamName;
        private String partitionKey;
        private byte[] payload;
        private int count;

        @Override
        public void append(String streamName, String partitionKey, long timestampMs, byte[] payload) {
            this.streamName = streamName;
            this.partitionKey = partitionKey;
            this.payload = payload;
            this.count++;
        }
    }

    /**
     * Runs one {@code data.json} case: builds a {@link SignalUpdate} from the input, publishes it
     * through a live {@link DataFacade}, and reports what reached the wire ({@code topic}/
     * {@code route}/{@code body}, plus {@code partitionKey} for a stream route) or
     * {@code {throws:true}} when the facade rejected it (missing {@code signal.id}, no samples, a
     * quality-only sample).
     */
    static JsonObject runDataCase(JsonObject input) {
        String instanceId = input.has("instance") ? input.get("instance").getAsString() : "kep1";
        MockMessagingService messaging = new MockMessagingService();
        RecordingStreamSink sink = new RecordingStreamSink();
        MockConfigurationService config = new MockConfigurationService();
        config.setComponentIdentity(FACADE_IDENTITY);
        Uns uns = new Uns(FACADE_IDENTITY.withInstance(instanceId), false);
        DataFacade facade = new DataFacade(config, instanceId, uns, messaging, sink, FIXED_CLOCK);

        SignalUpdate.Builder builder = new SignalUpdate.Builder(
                input.has("signalId") && !input.get("signalId").isJsonNull()
                        ? input.get("signalId").getAsString() : null);
        if (has(input, "signalName")) {
            builder.name(input.get("signalName").getAsString());
        }
        if (input.has("signalAddress") && input.get("signalAddress").isJsonObject()) {
            builder.address(input.getAsJsonObject("signalAddress"));
        }
        if (input.has("device") && input.get("device").isJsonObject()) {
            builder.device(input.getAsJsonObject("device"));
        }
        if (has(input, "signalPath")) {
            builder.signalPath(input.get("signalPath").getAsString());
        }
        if (input.has("samples")) {
            for (JsonElement sampleEl : input.getAsJsonArray("samples")) {
                JsonObject s = sampleEl.getAsJsonObject();
                Object value = s.has("value") ? s.get("value") : null;
                Quality quality = has(s, "quality") ? Quality.fromWire(s.get("quality").getAsString()) : null;
                String qualityRaw = has(s, "qualityRaw") ? s.get("qualityRaw").getAsString() : null;
                String sourceTs = has(s, "sourceTs") ? s.get("sourceTs").getAsString() : null;
                String serverTs = has(s, "serverTs") ? s.get("serverTs").getAsString() : null;
                builder.addSample(new SignalUpdate.Sample(value, quality, qualityRaw, sourceTs, serverTs));
            }
        }
        if (has(input, "override")) {
            builder.via(Channel.fromConfig(input.get("override").getAsString()));
        }

        try {
            facade.publish(builder.build());
        } catch (IllegalArgumentException e) {
            // UnsValidationException (a bad channel token) is itself an IllegalArgumentException.
            JsonObject out = new JsonObject();
            out.addProperty("throws", true);
            return out;
        }

        JsonObject out = new JsonObject();
        List<MockMessagingService.PublishedMessage> published = messaging.getPublishedMessages();
        if (!published.isEmpty()) {
            MockMessagingService.PublishedMessage pm = published.get(0);
            out.addProperty("topic", pm.topic);
            out.addProperty("route", pm.qos == null ? "local" : "northbound");
            out.add("body", pm.message.toDict().getAsJsonObject("body"));
        } else {
            String path = has(input, "signalPath") ? input.get("signalPath").getAsString()
                    : input.get("signalId").getAsString();
            out.addProperty("topic", uns.topic(UnsClass.DATA, facade.channelToken(path)));
            out.addProperty("route", "stream:" + sink.streamName);
            out.addProperty("partitionKey", sink.partitionKey);
            out.add("body", Message.fromBytes(sink.payload).toDict().getAsJsonObject("body"));
        }
        return out;
    }

    /** Runs one {@code evt.json} case through a live {@link EventsFacade}; reports {topic, route, body}. */
    static JsonObject runEvtCase(JsonObject input) {
        MockMessagingService messaging = new MockMessagingService();
        MockConfigurationService config = new MockConfigurationService();
        config.setComponentIdentity(FACADE_IDENTITY);
        Uns uns = new Uns(FACADE_IDENTITY, false);
        EventsFacade facade = new EventsFacade(config, "main", uns, messaging, FIXED_CLOCK);
        if (has(input, "override")) {
            facade = facade.via(Channel.fromConfig(input.get("override").getAsString()));
        }

        String kind = input.get("kind").getAsString();
        String type = input.get("type").getAsString();
        String message = has(input, "message") ? input.get("message").getAsString() : null;
        JsonObject context = input.has("context") && input.get("context").isJsonObject()
                ? input.getAsJsonObject("context") : null;
        Severity severity = has(input, "severity") ? Severity.fromWire(input.get("severity").getAsString()) : null;

        switch (kind) {
            case "emit" -> {
                if (severity == null) {
                    facade.emit(type, message);
                } else {
                    facade.emit(severity, type, message, context);
                }
            }
            case "raise" -> {
                if (severity == null) {
                    facade.raiseAlarm(type, message, context);
                } else {
                    facade.raiseAlarm(severity, type, message, context);
                }
            }
            case "clear" -> {
                if (severity == null) {
                    facade.clearAlarm(type, context);
                } else {
                    facade.clearAlarm(severity, type, context);
                }
            }
            default -> throw new IllegalArgumentException("unknown evt kind: " + kind);
        }

        MockMessagingService.PublishedMessage pm = messaging.getPublishedMessages().get(0);
        JsonObject out = new JsonObject();
        out.addProperty("topic", pm.topic);
        out.addProperty("route", pm.qos == null ? "local" : "northbound");
        out.add("body", pm.message.toDict().getAsJsonObject("body"));
        return out;
    }

    /** Runs one {@code app.json} case through a live {@link AppFacade}; reports {topic, route, body}. */
    static JsonObject runAppCase(JsonObject input) {
        MockMessagingService messaging = new MockMessagingService();
        MockConfigurationService config = new MockConfigurationService();
        config.setComponentIdentity(FACADE_IDENTITY);
        Uns uns = new Uns(FACADE_IDENTITY, false);
        AppFacade facade = new AppFacade(config, "main", uns, messaging);

        String name = input.get("name").getAsString();
        String channel = input.get("channel").getAsString();
        JsonObject body = input.has("body") && input.get("body").isJsonObject()
                ? input.getAsJsonObject("body") : new JsonObject();
        Channel routing = has(input, "override") ? Channel.fromConfig(input.get("override").getAsString()) : null;
        facade.publish(name, channel, body, routing);

        MockMessagingService.PublishedMessage pm = messaging.getPublishedMessages().get(0);
        JsonObject out = new JsonObject();
        out.addProperty("topic", pm.topic);
        out.addProperty("route", pm.qos == null ? "local" : "northbound");
        out.add("body", pm.message.toDict().getAsJsonObject("body"));
        return out;
    }

    /** True when {@code key} is present and not JSON null. */
    private static boolean has(JsonObject obj, String key) {
        return obj.has(key) && !obj.get(key).isJsonNull();
    }
}
