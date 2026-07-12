/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.commands;

import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.test.MockConfigurationService;
import com.mbreissi.edgecommons.test.MockMessagingService;
import com.mbreissi.edgecommons.uns.UnsValidationException;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import com.mbreissi.edgecommons.heartbeat.InstanceConnectivity;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Deterministic unit tests for the {@link CommandInbox} (DESIGN-uns §9.5, the minimal
 * {@code commands()} facade — edge-console slice S2) over the mock messaging/config seams:
 *
 * <ul>
 *   <li>{@code start()} subscribes exactly the own-inbox wildcard
 *       ({@code ecv1/{device}/{component}/main/cmd/#}) on the primary connection;</li>
 *   <li>each built-in verb dispatches and replies with the pinned body shape — {@code ping}
 *       (status + uptime), {@code reload-config} (ack / {@code RELOAD_FAILED}),
 *       {@code get-configuration} (redacted config / {@code NO_CONFIG});</li>
 *   <li>replies go to the request's {@code reply_to} with the request's
 *       {@code correlation_id} and the responder's identity;</li>
 *   <li>custom verbs register/dispatch (namespaced verbs included), cannot shadow built-ins or
 *       each other, and unregister; coded ({@link CommandException}) vs uncoded
 *       ({@code HANDLER_ERROR}) failures;</li>
 *   <li>unknown verbs get an {@code UNKNOWN_VERB} error reply (requests) or are ignored
 *       (fire-and-forget); no-{@code reply_to} commands run the handler without a reply;</li>
 *   <li>malformed payloads (name mismatch, headerless, null) and the delegated
 *       {@code set-config} verb are ignored — never replied to, never a crash;</li>
 *   <li>{@code close()} unsubscribes the inbox and stops dispatch; lifecycle is
 *       idempotent; a missing resolved identity disables the inbox.</li>
 * </ul>
 */
class CommandInboxTest {

    /** The default mock identity: device {@code test-thing}, component {@code TestComponent}. */
    private static final String INBOX_FILTER = "ecv1/test-thing/TestComponent/main/cmd/#";
    private static final String REPLY_TO = "edgecommons/reply-test-1";

    private MockConfigurationService config;
    private MockMessagingService messaging;
    private AtomicLong uptime;
    private AtomicBoolean reloadResult;
    private AtomicReference<JsonObject> redactedConfig;
    private CommandInbox inbox;

    @BeforeEach
    void setUp() {
        config = new MockConfigurationService();
        messaging = new MockMessagingService();
        uptime = new AtomicLong(42);
        reloadResult = new AtomicBoolean(true);
        redactedConfig = new AtomicReference<>(
                JsonParser.parseString("{\"component\":{\"global\":{\"v\":1}}}").getAsJsonObject());
        inbox = new CommandInbox(config, messaging,
                uptime::get, reloadResult::get, redactedConfig::get);
    }

    private static String topic(String verb) {
        return "ecv1/test-thing/TestComponent/main/cmd/" + verb;
    }

    /** A well-formed request for a verb: {@code header.name} = verb, pinned {@code reply_to}. */
    private static Message request(String verb) {
        Message message = MessageBuilder.create(verb, "1.0").withPayload(new JsonObject()).build();
        message.makeRequest(REPLY_TO);
        return message;
    }

    /** A well-formed fire-and-forget command (no {@code reply_to}). */
    private static Message notification(String verb) {
        return MessageBuilder.create(verb, "1.0").withPayload(new JsonObject()).build();
    }

    /** The single recorded reply (topic must be the request's {@code reply_to}). */
    private JsonObject onlyReplyBody() {
        assertEquals(1, messaging.getPublishedMessages().size(), "exactly one reply expected");
        MockMessagingService.PublishedMessage published = messaging.getPublishedMessages().get(0);
        assertEquals(REPLY_TO, published.topic, "the reply must go to the request's reply_to");
        return published.message.toDict().getAsJsonObject("body");
    }

    private static void assertVerb(JsonArray verbs, String verb, boolean builtIn) {
        for (int i = 0; i < verbs.size(); i++) {
            JsonObject entry = verbs.get(i).getAsJsonObject();
            if (verb.equals(entry.get("verb").getAsString())) {
                assertEquals(builtIn, entry.get("builtIn").getAsBoolean(),
                        "builtIn flag for " + verb);
                return;
            }
        }
        throw new AssertionError("verb not present in describe output: " + verb);
    }

    // ===================== subscription lifecycle =====================

    @Test
    void startSubscribesTheOwnInboxWildcard() {
        inbox.start();
        assertEquals(Set.of(INBOX_FILTER), messaging.getSubscribedTopics(),
                "start() must subscribe exactly the own-inbox cmd wildcard");
    }

    @Test
    void startIsIdempotent() {
        inbox.start();
        inbox.start();
        assertEquals(Set.of(INBOX_FILTER), messaging.getSubscribedTopics());
    }

    @Test
    void missingIdentityDisablesTheInbox() {
        config.setComponentIdentity(null); // the mock/test bring-up case
        inbox.start();
        assertTrue(messaging.getSubscribedTopics().isEmpty(),
                "no resolved identity -> no inbox subscription (WARN + disabled)");
        assertDoesNotThrow(inbox::close);
    }

    @Test
    void closeUnsubscribesAndStopsDispatch() {
        inbox.start();
        inbox.close();
        assertTrue(messaging.getSubscribedTopics().isEmpty(),
                "close() must unsubscribe the inbox (unsubscribe-before-exit)");
        // A late (queued) delivery after close is ignored.
        messaging.simulateMessage(topic(CommandInbox.PING), request(CommandInbox.PING));
        assertTrue(messaging.getPublishedMessages().isEmpty());
    }

    @Test
    void closeIsIdempotentAndStartAfterCloseIsANoOp() {
        inbox.start();
        inbox.close();
        assertDoesNotThrow(inbox::close);
        inbox.start(); // closed -> must not resubscribe
        assertTrue(messaging.getSubscribedTopics().isEmpty());
    }

    // ===================== built-in verbs =====================

    @Test
    void pingRepliesStatusAndUptime() {
        uptime.set(1234);
        inbox.start();
        messaging.simulateMessage(topic(CommandInbox.PING), request(CommandInbox.PING));
        JsonObject body = onlyReplyBody();
        assertTrue(body.get("ok").getAsBoolean());
        JsonObject result = body.getAsJsonObject("result");
        assertEquals("RUNNING", result.get("status").getAsString());
        assertEquals(1234, result.get("uptimeSecs").getAsLong());
    }

    /** With no provider registered — a plain service — `status` answers exactly as `ping` does. */
    @Test
    void statusWithoutAProviderAnswersLikePingAndOmitsInstances() {
        uptime.set(77);
        inbox.start();
        messaging.simulateMessage(topic(CommandInbox.STATUS), request(CommandInbox.STATUS));
        JsonObject result = onlyReplyBody().getAsJsonObject("result");
        assertEquals("RUNNING", result.get("status").getAsString());
        assertEquals(77, result.get("uptimeSecs").getAsLong());
        assertFalse(result.has("instances"),
                "a component with no instances must omit the section, not emit an empty array");
    }

    /** The pulled answer is the provider sample — the same one the state keepalive pushes. */
    @Test
    void statusReturnsTheProviderSampleIncludingStateAndAttributes() {
        JsonObject attrs = new JsonObject();
        attrs.add("capabilities", JsonParser.parseString("[\"ptz\",\"snapshot\"]"));
        attrs.addProperty("lastError", "CAMERA_UNAVAILABLE");

        CommandInbox withInstances = new CommandInbox(config, messaging,
                uptime::get, reloadResult::get, redactedConfig::get,
                () -> List.of(
                        InstanceConnectivity.of("cam-01", true).withState("ONLINE"),
                        new InstanceConnectivity("cam-02", false, "BACKOFF", "connect timed out",
                                Map.of("capabilities", attrs.get("capabilities"),
                                        "lastError", attrs.get("lastError")))));
        withInstances.start();
        messaging.simulateMessage(topic(CommandInbox.STATUS), request(CommandInbox.STATUS));

        JsonObject result = onlyReplyBody().getAsJsonObject("result");
        JsonArray instances = result.getAsJsonArray("instances");
        assertEquals(2, instances.size());

        JsonObject first = instances.get(0).getAsJsonObject();
        assertEquals("cam-01", first.get("instance").getAsString());
        assertTrue(first.get("connected").getAsBoolean());
        assertEquals("ONLINE", first.get("state").getAsString());
        assertFalse(first.has("attributes"), "an empty attribute bag is omitted");

        JsonObject second = instances.get(1).getAsJsonObject();
        assertFalse(second.get("connected").getAsBoolean());
        assertEquals("BACKOFF", second.get("state").getAsString());
        assertEquals("connect timed out", second.get("detail").getAsString());
        assertEquals("CAMERA_UNAVAILABLE",
                second.getAsJsonObject("attributes").get("lastError").getAsString());
    }

    /**
     * A throwing connectivity source must never crash the inbox — it degrades to the standard
     * uncoded-failure reply. (In production the source is {@code Heartbeat::sampleInstanceConnectivity},
     * which swallows a component's provider bug and yields an empty list, so `status` still answers;
     * this asserts the inbox is safe even if a caller wires a raw throwing supplier.)
     */
    @Test
    void statusSurvivesAThrowingProvider() {
        CommandInbox throwing = new CommandInbox(config, messaging,
                uptime::get, reloadResult::get, redactedConfig::get,
                () -> {
                    throw new IllegalStateException("provider blew up");
                });
        throwing.start();
        assertDoesNotThrow(() -> messaging.simulateMessage(topic(CommandInbox.STATUS),
                request(CommandInbox.STATUS)));
        JsonObject body = onlyReplyBody();
        assertFalse(body.get("ok").getAsBoolean());
        assertEquals(CommandInbox.ERR_HANDLER_ERROR,
                body.getAsJsonObject("error").get("code").getAsString());
    }

    /** `status` is a built-in: a component cannot shadow it. */
    @Test
    void statusCannotBeShadowedByACustomVerb() {
        assertThrows(IllegalArgumentException.class,
                () -> inbox.register(CommandInbox.STATUS, req -> null));
    }

    @Test
    void replyCarriesTheRequestCorrelationIdVerbNameAndResponderIdentity() {
        inbox.start();
        Message ping = request(CommandInbox.PING);
        messaging.simulateMessage(topic(CommandInbox.PING), ping);
        MockMessagingService.PublishedMessage published = messaging.getPublishedMessages().get(0);
        assertEquals(ping.getHeader().getCorrelationId(),
                published.message.getHeader().getCorrelationId(),
                "the reply must carry the request's correlation_id");
        assertEquals(CommandInbox.PING, published.message.getHeader().getName(),
                "the reply header.name is the verb");
        assertEquals(CommandInbox.CMD_MESSAGE_VERSION, published.message.getHeader().getVersion());
        assertNotNull(published.message.toDict().get("identity"),
                "the reply is config-stamped with the responder's identity");
    }

    @Test
    void reloadConfigRepliesAckOnSuccess() {
        inbox.start();
        messaging.simulateMessage(topic(CommandInbox.RELOAD_CONFIG),
                request(CommandInbox.RELOAD_CONFIG));
        JsonObject body = onlyReplyBody();
        assertTrue(body.get("ok").getAsBoolean());
        assertTrue(body.getAsJsonObject("result").get("reloaded").getAsBoolean());
    }

    @Test
    void reloadConfigRepliesReloadFailedOnFailure() {
        reloadResult.set(false);
        inbox.start();
        messaging.simulateMessage(topic(CommandInbox.RELOAD_CONFIG),
                request(CommandInbox.RELOAD_CONFIG));
        JsonObject body = onlyReplyBody();
        assertFalse(body.get("ok").getAsBoolean());
        assertEquals(CommandInbox.ERR_RELOAD_FAILED,
                body.getAsJsonObject("error").get("code").getAsString());
        assertFalse(body.getAsJsonObject("error").get("message").getAsString().isEmpty());
    }

    @Test
    void getConfigurationRepliesTheRedactedEffectiveConfig() {
        inbox.start();
        messaging.simulateMessage(topic(CommandInbox.GET_CONFIGURATION),
                request(CommandInbox.GET_CONFIGURATION));
        JsonObject body = onlyReplyBody();
        assertTrue(body.get("ok").getAsBoolean());
        assertEquals(redactedConfig.get(),
                body.getAsJsonObject("result").getAsJsonObject("config"),
                "get-configuration must return the redacted effective config (Flow B)");
    }

    @Test
    void getConfigurationRepliesNoConfigWhenUnavailable() {
        redactedConfig.set(null);
        inbox.start();
        messaging.simulateMessage(topic(CommandInbox.GET_CONFIGURATION),
                request(CommandInbox.GET_CONFIGURATION));
        JsonObject body = onlyReplyBody();
        assertFalse(body.get("ok").getAsBoolean());
        assertEquals(CommandInbox.ERR_NO_CONFIG,
                body.getAsJsonObject("error").get("code").getAsString());
    }

    @Test
    void describeIncludesBuiltInsCustomVerbsAndPanels() {
        JsonObject panel = JsonParser.parseString("""
                {
                  "id": "address-space",
                  "title": "Address Space",
                  "order": 20,
                  "widgets": [
                    {
                      "kind": "treeBrowser",
                      "id": "address-space-tree",
                      "browseVerb": "sb/browse"
                    }
                  ]
                }
                """).getAsJsonObject();
        inbox.register("sb/browse", req -> new JsonObject());
        inbox.registerPanel(panel);

        inbox.start();
        messaging.simulateMessage(topic(CommandInbox.DESCRIBE), request(CommandInbox.DESCRIBE));

        JsonObject body = onlyReplyBody();
        assertTrue(body.get("ok").getAsBoolean());
        JsonObject result = body.getAsJsonObject("result");
        assertEquals(CommandInbox.DESCRIBE_SCHEMA_VERSION,
                result.get("schemaVersion").getAsString());
        assertTrue(result.get("digest").getAsString().matches("sha256:[0-9a-f]{64}"));
        assertNotNull(result.getAsJsonObject("component"));

        JsonArray verbs = result.getAsJsonArray("commands");
        assertVerb(verbs, CommandInbox.PING, true);
        assertVerb(verbs, CommandInbox.DESCRIBE, true);
        assertVerb(verbs, CommandInbox.GET_CONFIGURATION, true);
        assertVerb(verbs, CommandInbox.RELOAD_CONFIG, true);
        assertVerb(verbs, "sb/browse", false);

        JsonObject panels = result.getAsJsonObject("panels");
        assertEquals(CommandInbox.PANELS_SCHEMA_VERSION,
                panels.get("schemaVersion").getAsString());
        assertEquals("TestComponent", panels.get("provider").getAsString());
        assertEquals("descriptor", panels.get("renderer").getAsString());
        assertEquals("address-space", panels.get("defaultView").getAsString());
        JsonArray views = panels.getAsJsonArray("views");
        assertEquals(1, views.size());
        assertEquals(panel, views.get(0).getAsJsonObject());
    }

    // ===================== custom verbs (the registration seam) =====================

    @Test
    void customVerbRegistersAndDispatches() {
        inbox.start(); // registration after start needs no new subscription
        inbox.register("restart-pipeline", req -> {
            JsonObject result = new JsonObject();
            result.addProperty("restarted", true);
            return result;
        });
        messaging.simulateMessage(topic("restart-pipeline"), request("restart-pipeline"));
        JsonObject body = onlyReplyBody();
        assertTrue(body.get("ok").getAsBoolean());
        assertTrue(body.getAsJsonObject("result").get("restarted").getAsBoolean());
    }

    @Test
    void namespacedCustomVerbDispatches() {
        inbox.register("sb/status", req -> null); // null result -> empty ack
        inbox.start();
        messaging.simulateMessage(topic("sb/status"), request("sb/status"));
        JsonObject body = onlyReplyBody();
        assertTrue(body.get("ok").getAsBoolean());
        assertEquals(new JsonObject(), body.getAsJsonObject("result"),
                "a null handler result must reply an empty result object");
    }

    @Test
    void handlerCommandExceptionKeepsItsCode() {
        inbox.register("guarded", req -> {
            throw new CommandException("NOT_ALLOWED", "operator role required");
        });
        inbox.start();
        messaging.simulateMessage(topic("guarded"), request("guarded"));
        JsonObject body = onlyReplyBody();
        assertFalse(body.get("ok").getAsBoolean());
        assertEquals("NOT_ALLOWED", body.getAsJsonObject("error").get("code").getAsString());
        assertEquals("operator role required",
                body.getAsJsonObject("error").get("message").getAsString());
    }

    @Test
    void handlerUncodedExceptionMapsToHandlerError() {
        inbox.register("boomy", req -> {
            throw new IllegalStateException("boom");
        });
        inbox.start();
        messaging.simulateMessage(topic("boomy"), request("boomy"));
        JsonObject body = onlyReplyBody();
        assertFalse(body.get("ok").getAsBoolean());
        assertEquals(CommandInbox.ERR_HANDLER_ERROR,
                body.getAsJsonObject("error").get("code").getAsString());
    }

    @Test
    void registerRejectsShadowingAndInvalidVerbs() {
        assertThrows(IllegalArgumentException.class,
                () -> inbox.register(CommandInbox.PING, req -> null),
                "a built-in verb cannot be shadowed");
        assertThrows(IllegalArgumentException.class,
                () -> inbox.register(CommandInbox.SET_CONFIG_VERB, req -> null),
                "a delegated verb cannot be registered");
        inbox.register("mine", req -> null);
        assertThrows(IllegalArgumentException.class, () -> inbox.register("mine", req -> null),
                "an already-registered verb cannot be re-registered");
        assertThrows(UnsValidationException.class, () -> inbox.register("bad+verb", req -> null),
                "verb tokens must pass the topic token rule");
        assertThrows(UnsValidationException.class, () -> inbox.register("sb//x", req -> null),
                "empty namespace tokens are rejected");
    }

    @Test
    void unregisterRemovesCustomVerbsButNeverBuiltIns() {
        inbox.register("mine", req -> null);
        assertTrue(inbox.verbs().contains("mine"));
        inbox.unregister("mine");
        assertFalse(inbox.verbs().contains("mine"));
        assertDoesNotThrow(() -> inbox.unregister("mine")); // unknown -> no-op
        assertThrows(IllegalArgumentException.class,
                () -> inbox.unregister(CommandInbox.RELOAD_CONFIG));
        // The unregistered verb now gets the unknown-verb error.
        inbox.start();
        messaging.simulateMessage(topic("mine"), request("mine"));
        assertEquals(CommandInbox.ERR_UNKNOWN_VERB,
                onlyReplyBody().getAsJsonObject("error").get("code").getAsString());
    }

    @Test
    void verbsSnapshotContainsBuiltInsAndCustoms() {
        inbox.register("mine", req -> null);
        assertEquals(Set.of(CommandInbox.PING, CommandInbox.RELOAD_CONFIG,
                CommandInbox.GET_CONFIGURATION, CommandInbox.DESCRIBE, CommandInbox.STATUS, "mine"),
                inbox.verbs());
    }

    @Test
    void panelsSnapshotContainsRegisteredViews() {
        JsonObject panel = JsonParser.parseString("""
                {"id":"overview","title":"Overview","scope":"component"}
                """).getAsJsonObject();
        inbox.registerPanel(panel);
        List<JsonObject> snapshot = inbox.panels();
        assertEquals(List.of(panel), snapshot);

        snapshot.get(0).addProperty("title", "Mutated");
        assertEquals("Overview", inbox.panels().get(0).get("title").getAsString(),
                "panels() must return a snapshot copy");
    }

    @Test
    void registerPanelRejectsInvalidPanelsAndDuplicateIds() {
        assertThrows(NullPointerException.class, () -> inbox.registerPanel(null),
                "panel must be a JSON object");
        assertThrows(IllegalArgumentException.class,
                () -> inbox.registerPanel(JsonParser.parseString("""
                        {"title":"Overview"}
                        """).getAsJsonObject()),
                "id is required");
        assertThrows(IllegalArgumentException.class,
                () -> inbox.registerPanel(JsonParser.parseString("""
                        {"id":"","title":"Overview"}
                        """).getAsJsonObject()),
                "id must be non-empty");
        assertThrows(IllegalArgumentException.class,
                () -> inbox.registerPanel(JsonParser.parseString("""
                        {"id":7,"title":"Overview"}
                        """).getAsJsonObject()),
                "id must be a string");
        assertThrows(IllegalArgumentException.class,
                () -> inbox.registerPanel(JsonParser.parseString("""
                        {"id":"overview"}
                        """).getAsJsonObject()),
                "title is required");
        assertThrows(IllegalArgumentException.class,
                () -> inbox.registerPanel(JsonParser.parseString("""
                        {"id":"overview","title":""}
                        """).getAsJsonObject()),
                "title must be non-empty");

        inbox.registerPanel(JsonParser.parseString("""
                {"id":"overview","title":"Overview"}
                """).getAsJsonObject());
        assertThrows(IllegalArgumentException.class,
                () -> inbox.registerPanel(JsonParser.parseString("""
                        {"id":"overview","title":"Different"}
                        """).getAsJsonObject()),
                "duplicate ids are rejected");
    }

    // ===================== unknown / fire-and-forget / malformed =====================

    @Test
    void unknownVerbRequestGetsAnUnknownVerbErrorReply() {
        inbox.start();
        messaging.simulateMessage(topic("no-such-verb"), request("no-such-verb"));
        JsonObject body = onlyReplyBody();
        assertFalse(body.get("ok").getAsBoolean());
        assertEquals(CommandInbox.ERR_UNKNOWN_VERB,
                body.getAsJsonObject("error").get("code").getAsString());
    }

    @Test
    void unknownFireAndForgetVerbIsIgnored() {
        inbox.start();
        messaging.simulateMessage(topic("no-such-verb"), notification("no-such-verb"));
        assertTrue(messaging.getPublishedMessages().isEmpty(),
                "an unknown fire-and-forget verb must not be replied to");
    }

    @Test
    void noReplyToRunsTheHandlerWithoutReplying() {
        boolean[] ran = {false};
        inbox.register("do-it", req -> {
            ran[0] = true;
            return null;
        });
        inbox.start();
        messaging.simulateMessage(topic("do-it"), notification("do-it"));
        assertTrue(ran[0], "a fire-and-forget command must still run the handler");
        assertTrue(messaging.getPublishedMessages().isEmpty(), "…but never reply");
    }

    @Test
    void fireAndForgetHandlerFailureIsLoggedOnly() {
        inbox.register("do-it", req -> {
            throw new CommandException("NOPE", "nope");
        });
        inbox.start();
        assertDoesNotThrow(() -> messaging.simulateMessage(topic("do-it"), notification("do-it")));
        assertTrue(messaging.getPublishedMessages().isEmpty());
    }

    @Test
    void malformedPayloadsAreIgnoredWithoutReplyAndNeverCrash() {
        inbox.start();
        // header.name does not equal the topic verb (foreign convention on a cmd topic).
        messaging.simulateMessage(topic(CommandInbox.PING), request("something-else"));
        // A raw (headerless) envelope - junk JSON on the inbox.
        messaging.simulateMessage(topic(CommandInbox.PING),
                MessageBuilder.fromObject(new JsonObject()));
        // A null message must not crash the callback either.
        assertDoesNotThrow(() -> messaging.simulateMessage(topic(CommandInbox.PING), null));
        assertTrue(messaging.getPublishedMessages().isEmpty(),
                "malformed/foreign payloads must never be replied to");
    }

    @Test
    void delegatedSetConfigIsIgnoredEvenAsARequest() {
        inbox.start();
        messaging.simulateMessage(topic(CommandInbox.SET_CONFIG_VERB),
                request(CommandInbox.SET_CONFIG_VERB));
        assertTrue(messaging.getPublishedMessages().isEmpty(),
                "set-config is owned by the CONFIG_COMPONENT subscription - never dispatched"
                        + " or replied to here");
    }

    @Test
    void bareCmdParentLevelDeliveryIsIgnored() {
        inbox.start();
        // MQTT "#" also matches the parent level (".../cmd") - nothing to dispatch there.
        messaging.simulateMessage("ecv1/test-thing/TestComponent/main/cmd",
                request(CommandInbox.PING));
        assertTrue(messaging.getPublishedMessages().isEmpty());
    }

    @Test
    void aFailingReplyPublishIsSwallowed() {
        MockMessagingService failing = new MockMessagingService() {
            @Override
            public void reply(Message request, Message reply) {
                throw new RuntimeException("broker down");
            }
        };
        CommandInbox failingInbox = new CommandInbox(config, failing,
                uptime::get, reloadResult::get, redactedConfig::get);
        failingInbox.start();
        assertDoesNotThrow(() -> failing.simulateMessage(topic(CommandInbox.PING),
                request(CommandInbox.PING)), "a failing reply publish must never crash dispatch");
        failingInbox.close();
    }
}
