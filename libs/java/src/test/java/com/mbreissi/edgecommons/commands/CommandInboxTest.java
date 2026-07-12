/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.commands;

import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.ReservedTopicException;
import com.mbreissi.edgecommons.test.MockConfigurationService;
import com.mbreissi.edgecommons.test.MockMessagingService;
import com.mbreissi.edgecommons.uns.UnsValidationException;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.util.List;
import java.util.Set;
import java.time.Duration;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.BooleanSupplier;

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

    private static void awaitCondition(BooleanSupplier condition, String message)
            throws InterruptedException {
        long deadline = System.nanoTime() + Duration.ofSeconds(3).toNanos();
        while (!condition.getAsBoolean() && System.nanoTime() < deadline) {
            Thread.sleep(10);
        }
        assertTrue(condition.getAsBoolean(), message);
    }

    // ===================== subscription lifecycle =====================

    @Test
    void startSubscribesTheOwnInboxWildcard() {
        inbox.start();
        assertEquals(Set.of(INBOX_FILTER), messaging.getSubscribedTopics(),
                "start() must subscribe exactly the own-inbox cmd wildcard");
        assertEquals(CommandInbox.StartupState.ACTIVE, inbox.startupStatus().state());
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
        assertEquals(CommandInbox.StartupState.FAILED, inbox.startupStatus().state());
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
        assertEquals(CommandInbox.StartupState.STOPPED, inbox.startupStatus().state());
    }

    @Test
    void deliveryRacingSubscriptionAcknowledgementIsDispatchedOnlyAfterActive() throws Exception {
        class RacingMessaging extends MockMessagingService {
            private boolean dispatchedInsideSubscribe;

            @Override
            public void subscribeAcknowledged(String topic,
                    java.util.function.BiConsumer<String, Message> handler,
                    int maxConcurrency, int maxMessages, Duration timeout) {
                super.subscribeAcknowledged(topic, handler, maxConcurrency, maxMessages, timeout);
                handler.accept(CommandInboxTest.topic(CommandInbox.PING),
                        CommandInboxTest.request(CommandInbox.PING));
                dispatchedInsideSubscribe = !getPublishedMessages().isEmpty();
            }
        }
        RacingMessaging racing = new RacingMessaging();
        CommandInbox subject = new CommandInbox(config, racing,
                uptime::get, reloadResult::get, redactedConfig::get);

        subject.start(Duration.ofSeconds(1));

        assertEquals(CommandInbox.StartupState.ACTIVE, subject.startupStatus().state());
        assertFalse(racing.dispatchedInsideSubscribe,
                "a delivery before acknowledgement must not see a partially-started inbox");
        awaitCondition(() -> racing.getPublishedMessages().size() == 1,
                "the activation gate must drain the acknowledged delivery after ACTIVE");
        racing.simulateMessage(topic(CommandInbox.PING), request(CommandInbox.PING));
        awaitCondition(() -> racing.getPublishedMessages().size() == 2,
                "post-ACTIVE delivery must dispatch after the retained delivery");
        subject.close();
    }

    @Test
    void failedAcknowledgementCleansPartialSubscriptionAndCanRestart() throws Exception {
        class FailsOnceMessaging extends MockMessagingService {
            private boolean first = true;

            @Override
            public void subscribeAcknowledged(String topic,
                    java.util.function.BiConsumer<String, Message> handler,
                    int maxConcurrency, int maxMessages, Duration timeout) {
                super.subscribeAcknowledged(topic, handler, maxConcurrency, maxMessages, timeout);
                if (first) {
                    first = false;
                    handler.accept(CommandInboxTest.topic(CommandInbox.PING),
                            CommandInboxTest.request(CommandInbox.PING));
                    throw new IllegalStateException("broker\nrefused\tsecret-control");
                }
            }
        }
        FailsOnceMessaging failsOnce = new FailsOnceMessaging();
        CommandInbox subject = new CommandInbox(config, failsOnce,
                uptime::get, reloadResult::get, redactedConfig::get);

        CommandInbox.StartupStatus failed = subject.start(Duration.ofSeconds(1));

        assertEquals(CommandInbox.StartupState.FAILED, failed.state());
        assertFalse(failed.error().contains("\n"));
        assertFalse(failed.error().contains("\t"));
        assertTrue(failsOnce.getSubscribedTopics().isEmpty(),
                "a failed acknowledged subscribe must clean any partial filter");
        assertTrue(failsOnce.getPublishedMessages().isEmpty(),
                "a delivery retained by a failed generation must be discarded");

        assertEquals(CommandInbox.StartupState.ACTIVE,
                subject.start(Duration.ofSeconds(1)).state());
        assertTrue(failsOnce.getPublishedMessages().isEmpty(),
                "a later generation must not drain the failed generation's delivery");
        failsOnce.simulateMessage(topic(CommandInbox.PING), request(CommandInbox.PING));
        awaitCondition(() -> failsOnce.getPublishedMessages().size() == 1,
                "the successful generation must dispatch new deliveries");
        subject.stop();
        assertEquals(CommandInbox.StartupState.STOPPED, subject.startupStatus().state());
        assertTrue(failsOnce.getSubscribedTopics().isEmpty());
        assertEquals(CommandInbox.StartupState.ACTIVE,
                subject.start(Duration.ofSeconds(1)).state());
        subject.close();
    }

    @Test
    void stopDuringAcknowledgementDiscardsPendingDeliveryAndStalesTheStart() throws Exception {
        class BlockingMessaging extends MockMessagingService {
            private final CountDownLatch callbackQueued = new CountDownLatch(1);
            private final CountDownLatch releaseAcknowledgement = new CountDownLatch(1);

            @Override
            public void subscribeAcknowledged(String topic,
                    java.util.function.BiConsumer<String, Message> handler,
                    int maxConcurrency, int maxMessages, Duration timeout) {
                super.subscribeAcknowledged(topic, handler, maxConcurrency, maxMessages, timeout);
                handler.accept(CommandInboxTest.topic(CommandInbox.PING),
                        CommandInboxTest.request(CommandInbox.PING));
                callbackQueued.countDown();
                try {
                    if (!releaseAcknowledgement.await(2, java.util.concurrent.TimeUnit.SECONDS)) {
                        throw new IllegalStateException("test acknowledgement was not released");
                    }
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    throw new IllegalStateException("test acknowledgement interrupted", e);
                }
            }
        }
        BlockingMessaging blocking = new BlockingMessaging();
        CommandInbox subject = new CommandInbox(config, blocking,
                uptime::get, reloadResult::get, redactedConfig::get);
        AtomicReference<CommandInbox.StartupStatus> returned = new AtomicReference<>();
        Thread starter = Thread.startVirtualThread(
                () -> returned.set(subject.start(Duration.ofSeconds(2))));
        assertTrue(blocking.callbackQueued.await(1, java.util.concurrent.TimeUnit.SECONDS));

        subject.stop();
        blocking.releaseAcknowledgement.countDown();
        starter.join(Duration.ofSeconds(2));

        assertEquals(CommandInbox.StartupState.STOPPED, returned.get().state());
        assertEquals(CommandInbox.StartupState.STOPPED, subject.startupStatus().state());
        assertTrue(blocking.getPublishedMessages().isEmpty());
        assertTrue(blocking.getSubscribedTopics().isEmpty());
        subject.close();
    }

    @Test
    void startupActivationQueueIsStrictlyBounded() throws Exception {
        class FloodingMessaging extends MockMessagingService {
            @Override
            public void subscribeAcknowledged(String topic,
                    java.util.function.BiConsumer<String, Message> handler,
                    int maxConcurrency, int maxMessages, Duration timeout) {
                super.subscribeAcknowledged(topic, handler, maxConcurrency, maxMessages, timeout);
                for (int i = 0; i < CommandInbox.MAX_PENDING_STARTUP_DELIVERIES + 1; i++) {
                    handler.accept(CommandInboxTest.topic(CommandInbox.PING),
                            CommandInboxTest.request(CommandInbox.PING));
                }
            }
        }
        FloodingMessaging flooding = new FloodingMessaging();
        CommandInbox subject = new CommandInbox(config, flooding,
                uptime::get, reloadResult::get, redactedConfig::get);

        subject.start(Duration.ofSeconds(1));

        awaitCondition(() -> flooding.getPublishedMessages().size()
                        == CommandInbox.MAX_PENDING_STARTUP_DELIVERIES,
                "the bounded activation queue must drain every retained delivery");
        subject.close();
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
                CommandInbox.GET_CONFIGURATION, CommandInbox.DESCRIBE, "mine"), inbox.verbs());
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

    // ===================== explicit outcomes and deferred replies =====================

    @Test
    void outcomeHandlersPreserveStandardImmediateWrappers() {
        JsonObject result = new JsonObject();
        result.addProperty("accepted", true);
        inbox.registerOutcome("outcome-ok", req -> CommandOutcome.success(result));
        inbox.start();

        messaging.simulateMessage(topic("outcome-ok"), request("outcome-ok"));
        JsonObject success = onlyReplyBody();
        assertTrue(success.get("ok").getAsBoolean());
        assertTrue(success.getAsJsonObject("result").get("accepted").getAsBoolean());

        messaging.clearPublishedMessages();
        inbox.registerOutcome("outcome-error",
                req -> CommandOutcome.error("CAMERA_BUSY", "camera is busy"));
        messaging.simulateMessage(topic("outcome-error"), request("outcome-error"));
        JsonObject error = onlyReplyBody();
        assertFalse(error.get("ok").getAsBoolean());
        assertEquals("CAMERA_BUSY", error.getAsJsonObject("error").get("code").getAsString());
        inbox.close();
    }

    @Test
    void anOutcomeHandlerMustReturnAnOutcomeAndItsCodedFailuresSurvive() {
        // The outcome contract is "return a non-null explicit outcome". A handler that returns
        // nothing is a bug in the component, and must surface as a coded error reply rather than
        // a swallowed no-reply (which would hang the caller until its request deadline).
        inbox.registerOutcome("outcome-null", req -> null);
        inbox.start();
        messaging.simulateMessage(topic("outcome-null"), request("outcome-null"));
        JsonObject body = onlyReplyBody();
        assertFalse(body.get("ok").getAsBoolean());
        assertEquals(CommandInbox.ERR_HANDLER_ERROR,
                body.getAsJsonObject("error").get("code").getAsString());

        // A coded failure thrown from an outcome handler keeps its code, exactly as for the
        // legacy handler surface.
        messaging.clearPublishedMessages();
        inbox.registerOutcome("outcome-coded", req -> {
            throw new CommandException("CAMERA_BUSY", "a capture is already running");
        });
        messaging.simulateMessage(topic("outcome-coded"), request("outcome-coded"));
        JsonObject coded = onlyReplyBody();
        assertFalse(coded.get("ok").getAsBoolean());
        assertEquals("CAMERA_BUSY", coded.getAsJsonObject("error").get("code").getAsString());
        assertEquals("a capture is already running",
                coded.getAsJsonObject("error").get("message").getAsString());
        inbox.close();
    }

    @Test
    void aProvisionalTokenThatIsNeverActivatedStillExpiresAndReleasesItsCapacity() throws Exception {
        // A handler may provision a token and then fail before activating it. The registry is hard
        // bounded, so such a token must not sit there holding a slot forever.
        CommandInbox.DeferredReply token = inbox.defer(request("capture"), Duration.ofMillis(40));
        assertEquals(CommandInbox.DeferredReplyState.PROVISIONAL, token.state());
        assertEquals(1, inbox.deferredReplySnapshot().active());

        awaitCondition(() -> token.state() == CommandInbox.DeferredReplyState.EXPIRED,
                "a provisional token must expire on its own lifetime timer");
        awaitCondition(() -> inbox.deferredReplySnapshot().active() == 0,
                "expiring a provisional token must release its registry slot");
        assertEquals(1, inbox.deferredReplySnapshot().expired());
        assertTrue(messaging.getPublishedMessages().isEmpty(),
                "a token that was never activated owes no reply");
        inbox.close();
    }

    @Test
    void aFailingComponentStoppingReplyStillCancelsTheTokenAndNeverBreaksShutdown() {
        MockMessagingService brokerDown = new MockMessagingService() {
            @Override
            public void publishConfirmed(String topic, byte[] encodedMessage,
                                         com.mbreissi.edgecommons.messaging.Qos qos,
                                         Duration timeout) {
                throw new RuntimeException("broker is gone");
            }
        };
        CommandInbox subject = new CommandInbox(config, brokerDown,
                uptime::get, reloadResult::get, redactedConfig::get);
        AtomicReference<CommandInbox.DeferredReply> tokenRef = new AtomicReference<>();
        subject.registerOutcome("shutdown", req -> {
            CommandInbox.DeferredReply token = subject.defer(req, Duration.ofSeconds(5));
            tokenRef.set(token);
            token.activate();
            return CommandOutcome.deferred(token);
        });
        subject.start();
        brokerDown.simulateMessage(topic("shutdown"), request("shutdown"));

        assertDoesNotThrow(subject::close,
                "an unreachable broker must not turn shutdown into a crash");

        assertEquals(CommandInbox.DeferredReplyState.CANCELLED_ON_SHUTDOWN,
                tokenRef.get().state(),
                "the token reaches its terminal state even when its stopping reply cannot be sent");
        assertEquals(0, subject.deferredReplySnapshot().active());
        assertEquals(1, subject.deferredReplySnapshot().cancelledOnShutdown());
    }

    @Test
    void registerOutcomeSharesDuplicateAndUnregisterRulesWithLegacyHandlers() {
        inbox.registerOutcome("explicit", req -> CommandOutcome.success(null));
        assertTrue(inbox.verbs().contains("explicit"));
        assertThrows(IllegalArgumentException.class,
                () -> inbox.register("explicit", req -> null));
        assertThrows(IllegalArgumentException.class,
                () -> inbox.registerOutcome(CommandInbox.PING,
                        req -> CommandOutcome.success(null)));
        inbox.unregister("explicit");
        assertFalse(inbox.verbs().contains("explicit"));
    }

    @Test
    void activatedDeferredSuppressesAutoReplyAndSettlesExactlyOnce() throws Exception {
        AtomicReference<CommandInbox.DeferredReply> tokenRef = new AtomicReference<>();
        inbox.registerOutcome("long-capture", req -> {
            CommandInbox.DeferredReply token = inbox.defer(req, Duration.ofSeconds(2));
            tokenRef.set(token);
            assertTrue(token.activate(), "durable acceptance activates the provisional token");
            return CommandOutcome.deferred(token);
        });
        inbox.start();
        Message command = request("long-capture");

        messaging.simulateMessage(topic("long-capture"), command);
        CommandInbox.DeferredReply token = tokenRef.get();
        assertNotNull(token);
        assertEquals(CommandInbox.DeferredReplyState.OPEN, token.state());
        assertTrue(messaging.getPublishedMessages().isEmpty(),
                "returning Deferred must suppress the automatic reply");

        JsonObject terminal = new JsonObject();
        terminal.addProperty("captureId", "cap-1");
        assertEquals(CommandInbox.SettlementResult.ACCEPTED,
                token.settleSuccess(terminal));
        assertEquals(CommandInbox.SettlementResult.ALREADY_SETTLED,
                token.settleError("TOO_LATE", "second settler loses"));
        awaitCondition(() -> token.state() == CommandInbox.DeferredReplyState.SETTLED,
                "confirmed deferred reply did not settle");

        JsonObject reply = onlyReplyBody();
        assertTrue(reply.get("ok").getAsBoolean());
        assertEquals("cap-1", reply.getAsJsonObject("result").get("captureId").getAsString());
        assertEquals(command.getHeader().getCorrelationId(),
                messaging.getPublishedMessages().get(0).message.getHeader().getCorrelationId());
        assertEquals(1, messaging.getConfirmedPublishes().size());
        assertEquals(0, inbox.deferredReplySnapshot().active());
        assertEquals(1, inbox.deferredReplySnapshot().settled());
        inbox.close();
    }

    @Test
    void postAcceptContinuationStartsOnlyAfterTheInboxAcceptsAnOpenToken() throws Exception {
        AtomicReference<CommandInbox.DeferredReply> tokenRef = new AtomicReference<>();
        CountDownLatch continuationStarted = new CountDownLatch(1);
        inbox.registerOutcome("post-accept", req -> {
            CommandInbox.DeferredReply token = inbox.defer(req, Duration.ofSeconds(2));
            tokenRef.set(token);
            assertTrue(token.activate());
            return CommandOutcome.deferredWithContinuation(token, () -> {
                continuationStarted.countDown();
                JsonObject terminal = new JsonObject();
                terminal.addProperty("captureId", "cap-post-accept");
                token.settleSuccess(terminal);
            });
        });
        inbox.start();

        messaging.simulateMessage(topic("post-accept"), request("post-accept"));

        assertTrue(continuationStarted.await(1, TimeUnit.SECONDS),
                "the inbox-owned continuation should run after deferred acceptance");
        awaitCondition(() -> tokenRef.get().state() == CommandInbox.DeferredReplyState.SETTLED,
                "post-accept continuation did not settle its guarded token");
        assertEquals("cap-post-accept",
                onlyReplyBody().getAsJsonObject("result").get("captureId").getAsString());
        inbox.close();
    }

    @Test
    void invalidPostAcceptTokenNeverStartsItsContinuation() throws Exception {
        AtomicBoolean continuationRan = new AtomicBoolean(false);
        inbox.registerOutcome("post-accept-invalid", req -> {
            CommandInbox.DeferredReply token = inbox.defer(req, Duration.ofSeconds(1));
            // Leave the token PROVISIONAL. The dispatcher must reject before scheduling.
            return CommandOutcome.deferredWithContinuation(token,
                    () -> continuationRan.set(true));
        });
        inbox.start();

        messaging.simulateMessage(topic("post-accept-invalid"), request("post-accept-invalid"));

        assertFalse(continuationRan.get(), "invalid deferred tokens must not start work");
        assertEquals(CommandInbox.ERR_HANDLER_ERROR,
                onlyReplyBody().getAsJsonObject("error").get("code").getAsString());
        inbox.close();
    }

    @Test
    void failedPostAcceptContinuationSettlesThroughTheGuardedErrorPath() throws Exception {
        AtomicReference<CommandInbox.DeferredReply> tokenRef = new AtomicReference<>();
        inbox.registerOutcome("post-accept-failure", req -> {
            CommandInbox.DeferredReply token = inbox.defer(req, Duration.ofSeconds(2));
            tokenRef.set(token);
            assertTrue(token.activate());
            return CommandOutcome.deferredWithContinuation(token, () -> {
                throw new IllegalStateException("simulated camera worker failure");
            });
        });
        inbox.start();

        messaging.simulateMessage(topic("post-accept-failure"), request("post-accept-failure"));

        awaitCondition(() -> tokenRef.get().state() == CommandInbox.DeferredReplyState.SETTLED,
                "failed post-accept continuation did not settle its token");
        assertEquals(CommandInbox.ERR_HANDLER_ERROR,
                onlyReplyBody().getAsJsonObject("error").get("code").getAsString());
        inbox.close();
    }

    @Test
    void concurrentDeferredSettlersHaveOneAtomicWinner() throws Exception {
        AtomicReference<CommandInbox.DeferredReply> tokenRef = new AtomicReference<>();
        inbox.registerOutcome("settle-race", req -> {
            CommandInbox.DeferredReply token = inbox.defer(req, Duration.ofSeconds(2));
            tokenRef.set(token);
            token.activate();
            return CommandOutcome.deferred(token);
        });
        inbox.start();
        messaging.simulateMessage(topic("settle-race"), request("settle-race"));

        CountDownLatch ready = new CountDownLatch(2);
        CountDownLatch go = new CountDownLatch(1);
        List<CommandInbox.SettlementResult> results = new CopyOnWriteArrayList<>();
        Runnable settle = () -> {
            ready.countDown();
            try {
                go.await();
                results.add(tokenRef.get().settleSuccess(new JsonObject()));
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
        };
        Thread first = Thread.startVirtualThread(settle);
        Thread second = Thread.startVirtualThread(settle);
        assertTrue(ready.await(1, java.util.concurrent.TimeUnit.SECONDS));
        go.countDown();
        first.join();
        second.join();

        assertEquals(1, results.stream()
                .filter(r -> r == CommandInbox.SettlementResult.ACCEPTED).count());
        assertEquals(1, results.stream()
                .filter(r -> r == CommandInbox.SettlementResult.ALREADY_SETTLED).count());
        awaitCondition(() -> tokenRef.get().state() == CommandInbox.DeferredReplyState.SETTLED,
                "winning settlement did not publish");
        assertEquals(1, messaging.getPublishedMessages().size());
        inbox.close();
    }

    @Test
    void settlementAndExpirationRaceProduceOneTerminalStateAndAtMostOneReply()
            throws Exception {
        AtomicReference<CommandInbox.DeferredReply> tokenRef = new AtomicReference<>();
        inbox.registerOutcome("expiry-race", req -> {
            CommandInbox.DeferredReply token = inbox.defer(req, Duration.ofMillis(100));
            tokenRef.set(token);
            token.activate();
            return CommandOutcome.deferred(token);
        });
        inbox.start();
        messaging.simulateMessage(topic("expiry-race"), request("expiry-race"));

        Thread.sleep(85);
        CommandInbox.SettlementResult result =
                tokenRef.get().settleSuccess(new JsonObject());
        awaitCondition(() -> {
            CommandInbox.DeferredReplyState state = tokenRef.get().state();
            return state == CommandInbox.DeferredReplyState.SETTLED
                    || state == CommandInbox.DeferredReplyState.EXPIRED;
        }, "settlement/expiration race did not terminate");

        CommandInbox.DeferredReplyState finalState = tokenRef.get().state();
        if (result == CommandInbox.SettlementResult.ACCEPTED) {
            assertTrue(finalState == CommandInbox.DeferredReplyState.SETTLED
                    || finalState == CommandInbox.DeferredReplyState.EXPIRED);
        } else {
            assertEquals(CommandInbox.SettlementResult.EXPIRED, result);
            assertEquals(CommandInbox.DeferredReplyState.EXPIRED, finalState);
        }
        assertTrue(messaging.getPublishedMessages().size() <= 1);
        assertEquals(0, inbox.deferredReplySnapshot().active());
        inbox.close();
    }

    @Test
    void provisionalOrForeignDeferredTokenIsRejectedAndDiscarded() {
        AtomicReference<CommandInbox.DeferredReply> tokenRef = new AtomicReference<>();
        inbox.registerOutcome("not-activated", req -> {
            CommandInbox.DeferredReply token = inbox.defer(req, Duration.ofSeconds(1));
            tokenRef.set(token);
            return CommandOutcome.deferred(token);
        });
        inbox.start();

        messaging.simulateMessage(topic("not-activated"), request("not-activated"));
        assertEquals(CommandInbox.DeferredReplyState.DISCARDED, tokenRef.get().state());
        JsonObject body = onlyReplyBody();
        assertFalse(body.get("ok").getAsBoolean());
        assertEquals(CommandInbox.ERR_HANDLER_ERROR,
                body.getAsJsonObject("error").get("code").getAsString());
        inbox.close();
    }

    @Test
    void deferRejectsMissingHostileOrUnboundedReplyMetadataBeforeProvisioning() throws Exception {
        CommandException missing = assertThrows(CommandException.class,
                () -> inbox.defer(notification("capture"), Duration.ofSeconds(1)));
        assertEquals(CommandInbox.ERR_REPLY_REQUIRED, missing.getCode());

        Message hostile = request("capture");
        hostile.makeRequest("ecv1/device/component/main/state");
        assertThrows(ReservedTopicException.class,
                () -> inbox.defer(hostile, Duration.ofSeconds(1)));
        assertThrows(IllegalArgumentException.class,
                () -> inbox.defer(request("capture"), Duration.ZERO));
        assertThrows(IllegalArgumentException.class,
                () -> inbox.defer(request("capture"), Duration.ofMillis(
                        CommandInbox.MAX_DEFERRED_REPLY_LIFETIME_MS + 1)));
        assertEquals(0, inbox.deferredReplySnapshot().active());

        inbox.close();
        CommandException stopping = assertThrows(CommandException.class,
                () -> inbox.defer(request("capture"), Duration.ofSeconds(1)));
        assertEquals(CommandInbox.ERR_COMPONENT_STOPPING, stopping.getCode());
    }

    @Test
    void openDeferredTokenExpiresOnTimerWithObservableDiagnosticState() throws Exception {
        AtomicReference<CommandInbox.DeferredReply> tokenRef = new AtomicReference<>();
        inbox.registerOutcome("expires", req -> {
            CommandInbox.DeferredReply token = inbox.defer(req, Duration.ofMillis(40));
            tokenRef.set(token);
            token.activate();
            return CommandOutcome.deferred(token);
        });
        inbox.start();
        messaging.simulateMessage(topic("expires"), request("expires"));

        awaitCondition(() -> tokenRef.get().state() == CommandInbox.DeferredReplyState.EXPIRED
                        && inbox.deferredReplySnapshot().active() == 0,
                "open deferred token did not expire and finish registry cleanup");
        CommandInbox.DeferredReplySnapshot snapshot = inbox.deferredReplySnapshot();
        assertEquals(0, snapshot.active());
        assertEquals(1, snapshot.expired());
        assertEquals(1, snapshot.openExpired());
        assertTrue(messaging.getPublishedMessages().isEmpty());
        inbox.close();
    }

    @Test
    void deferredReplyRetriesConfirmedTransportUntilSuccess() throws Exception {
        AtomicInteger attempts = new AtomicInteger();
        MockMessagingService flaky = new MockMessagingService() {
            @Override
            public void publishConfirmed(String topic, byte[] encodedMessage,
                                         com.mbreissi.edgecommons.messaging.Qos qos,
                                         Duration timeout) {
                if (attempts.incrementAndGet() < 3) {
                    throw new RuntimeException("temporary broker failure");
                }
                super.publishConfirmed(topic, encodedMessage, qos, timeout);
            }
        };
        CommandInbox retryInbox = new CommandInbox(config, flaky,
                uptime::get, reloadResult::get, redactedConfig::get);
        AtomicReference<CommandInbox.DeferredReply> tokenRef = new AtomicReference<>();
        retryInbox.registerOutcome("retry", req -> {
            CommandInbox.DeferredReply token = retryInbox.defer(req, Duration.ofSeconds(2));
            tokenRef.set(token);
            token.activate();
            return CommandOutcome.deferred(token);
        });
        retryInbox.start();
        flaky.simulateMessage(topic("retry"), request("retry"));

        assertEquals(CommandInbox.SettlementResult.ACCEPTED,
                tokenRef.get().settleSuccess(new JsonObject()));
        awaitCondition(() -> tokenRef.get().state() == CommandInbox.DeferredReplyState.SETTLED,
                "deferred reply did not settle after retries");
        assertEquals(3, attempts.get());
        assertEquals(1, flaky.getPublishedMessages().size());
        retryInbox.close();
    }

    @Test
    void closeAttemptsComponentStoppingThenCancelsOpenTokens() {
        AtomicReference<CommandInbox.DeferredReply> tokenRef = new AtomicReference<>();
        inbox.registerOutcome("shutdown", req -> {
            CommandInbox.DeferredReply token = inbox.defer(req, Duration.ofSeconds(2));
            tokenRef.set(token);
            token.activate();
            return CommandOutcome.deferred(token);
        });
        inbox.start();
        messaging.simulateMessage(topic("shutdown"), request("shutdown"));

        inbox.close();
        assertEquals(CommandInbox.DeferredReplyState.CANCELLED_ON_SHUTDOWN,
                tokenRef.get().state());
        JsonObject body = onlyReplyBody();
        assertFalse(body.get("ok").getAsBoolean());
        assertEquals(CommandInbox.ERR_COMPONENT_STOPPING,
                body.getAsJsonObject("error").get("code").getAsString());
        assertEquals(1, inbox.deferredReplySnapshot().cancelledOnShutdown());
        assertEquals(0, inbox.deferredReplySnapshot().active());
    }

    @Test
    void deferredRegistryIsHardBoundedAt1024() throws Exception {
        Message command = request("capacity");
        for (int i = 0; i < CommandInbox.MAX_DEFERRED_REPLIES; i++) {
            assertNotNull(inbox.defer(command, Duration.ofSeconds(5)));
        }
        CommandException full = assertThrows(CommandException.class,
                () -> inbox.defer(command, Duration.ofSeconds(5)));
        assertEquals(CommandInbox.ERR_DEFERRED_REPLY_CAPACITY, full.getCode());
        assertEquals(CommandInbox.MAX_DEFERRED_REPLIES,
                inbox.deferredReplySnapshot().active());
        assertEquals(1, inbox.deferredReplySnapshot().capacityRejected());
        inbox.close();
        assertEquals(0, inbox.deferredReplySnapshot().active());
    }
}
