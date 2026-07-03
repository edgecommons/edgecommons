/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.config.provider;

import com.mbreissi.ggcommons.messaging.Message;
import com.mbreissi.ggcommons.messaging.MessageBuilder;
import com.mbreissi.ggcommons.messaging.ReplyFuture;
import com.mbreissi.ggcommons.test.MockConfigurationService;
import com.mbreissi.ggcommons.test.MockMessagingService;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Behavior tests for {@link ConfigComponentProvider} on the UNS config rendezvous
 * (UNS-CANONICAL-DESIGN §4.3, D-U19 Flow A + the set-config push) and the pre-identity
 * bootstrap contract (§1.5):
 *
 * <ul>
 *   <li>The GET request goes to {@code ecv1/{device}/config/main/cmd/get-configuration} and
 *       self-identifies in the body with {@code {"component": "<short name>"}} — the envelope
 *       carries no identity because the {@code ConfigManager} does not exist yet.</li>
 *   <li>Every test builds the provider with a <b>null</b> {@code ConfigManager}, exactly like the
 *       production bootstrap ({@code ConfigManagerFactory} passes null) — the slice-1a flagged
 *       NPE regression guard.</li>
 *   <li>A pushed {@code set-config} on the component's own inbox
 *       {@code ecv1/{device}/{component}/main/cmd/set-config} applies via
 *       {@code applyConfig} once the manager is attached, and is dropped (not an NPE) before.</li>
 *   <li>The 3-attempt retry contract from the framework request deadline (§5) is unchanged:
 *       each retry issues a FRESH request (a settled future can never complete).</li>
 * </ul>
 */
class ConfigComponentProviderTest {

    private static final String EXPECTED_GET_TOPIC = "ecv1/test-thing/config/main/cmd/get-configuration";
    private static final String EXPECTED_SET_CONFIG_TOPIC = "ecv1/test-thing/Comp/main/cmd/set-config";

    /** MockMessagingService whose request() futures are scripted per attempt. */
    private static final class ScriptedMessaging extends MockMessagingService {
        final List<ReplyFuture> scripted = new ArrayList<>();
        final List<String> requestTopics = new ArrayList<>();
        final List<Message> requestMessages = new ArrayList<>();
        final AtomicInteger requests = new AtomicInteger();
        final AtomicInteger cancels = new AtomicInteger();

        @Override
        public ReplyFuture request(String topic, Message message) {
            requestTopics.add(topic);
            requestMessages.add(message);
            int n = requests.getAndIncrement();
            return scripted.get(Math.min(n, scripted.size() - 1));
        }

        @Override
        public void cancelRequest(ReplyFuture replyFuture) {
            cancels.incrementAndGet();
            if (replyFuture.trySettle()) {
                replyFuture.complete(null);
            }
        }
    }

    /** A ConfigManager stand-in recording what the set-config push handler applies. */
    private static final class CapturingConfigManager extends MockConfigurationService {
        final List<JsonObject> applied = new ArrayList<>();

        @Override
        public void applyConfig(JsonObject config) {
            applied.add(config);
        }
    }

    private static ReplyFuture timedOut() {
        ReplyFuture f = new ReplyFuture("ggcommons/reply-deadline");
        f.trySettle();
        f.completeExceptionally(new TimeoutException("request timed out (framework deadline)"));
        return f;
    }

    private static ReplyFuture replied(JsonObject body) {
        // A real reply is a full message envelope; loadConfiguration reads its "body".
        JsonObject envelope = new JsonObject();
        envelope.add("body", body);
        ReplyFuture f = new ReplyFuture("ggcommons/reply-ok");
        f.trySettle();
        f.complete(MessageBuilder.fromObject(envelope));
        return f;
    }

    /**
     * Builds the provider through the production path: a NULL ConfigManager (it does not exist
     * yet during config bootstrap) with the thing/component names from the platform inputs.
     */
    private static ConfigComponentProvider provider(ScriptedMessaging messaging) {
        return provider(messaging, "com.test.Comp", "test-thing");
    }

    private static ConfigComponentProvider provider(ScriptedMessaging messaging,
                                                    String componentName, String thingName) {
        return (ConfigComponentProvider) ConfigProviderBuilder.build(
                null, componentName, thingName,
                new String[]{"CONFIG_COMPONENT"}, messaging);
    }

    /** A fire-and-forget set-config push message whose body is the new configuration. */
    private static Message setConfigPush(JsonObject newConfig) {
        return MessageBuilder.create("SetConfig", "1.0").withPayload(newConfig).build();
    }

    // ----- Flow A: the GET rendezvous (D-U19) -----

    @Test
    void getRequestUsesTheUnsConfigRendezvousAndSelfIdentifiesInTheBody() {
        ScriptedMessaging messaging = new ScriptedMessaging();
        JsonObject cfg = new JsonObject();
        cfg.addProperty("component", "x");
        messaging.scripted.add(replied(cfg));

        provider(messaging).loadConfiguration();

        assertEquals(EXPECTED_GET_TOPIC, messaging.requestTopics.get(0),
                "the GET must target the reserved-by-convention 'config' logical component");
        JsonObject body = (JsonObject) messaging.requestMessages.get(0).getBody();
        assertEquals("Comp", body.get("component").getAsString(),
                "the requester must self-identify in the body with the SHORT component name");
    }

    @Test
    void loadReturnsTheReplyBodyOnFirstAttempt() {
        ScriptedMessaging messaging = new ScriptedMessaging();
        JsonObject cfg = new JsonObject();
        cfg.addProperty("component", "x");
        messaging.scripted.add(replied(cfg));

        JsonObject loaded = provider(messaging).loadConfiguration();

        assertEquals("x", loaded.get("component").getAsString());
        assertEquals(1, messaging.requests.get(), "one request suffices on the happy path");
    }

    @Test
    void topicsAreMintedFromSanitizedThingAndShortComponentTokens() {
        // Reserved topic characters (/ + #) in the platform inputs must be neutralized by the
        // normative token sanitizer, resolved WITHOUT any ConfigManager.
        ScriptedMessaging messaging = new ScriptedMessaging();
        JsonObject cfg = new JsonObject();
        messaging.scripted.add(replied(cfg));
        ConfigComponentProvider p = provider(messaging, "com.test.My/Comp+1", "plant#7/east");

        p.loadConfiguration();
        assertEquals("ecv1/plant_7_east/config/main/cmd/get-configuration",
                messaging.requestTopics.get(0));
        JsonObject body = (JsonObject) messaging.requestMessages.get(0).getBody();
        assertEquals("My_Comp_1", body.get("component").getAsString());

        // The set-config inbox uses the same sanitized tokens: an exact-topic push must land.
        CapturingConfigManager manager = new CapturingConfigManager();
        p.attachConfigManager(manager);
        JsonObject pushed = new JsonObject();
        pushed.addProperty("v", 2);
        messaging.simulateMessage("ecv1/plant_7_east/My_Comp_1/main/cmd/set-config", setConfigPush(pushed));
        assertEquals(1, manager.applied.size(), "the push must arrive on the sanitized inbox topic");
    }

    // ----- the set-config push (component's own inbox) -----

    @Test
    void pushedSetConfigAppliesOnceTheConfigManagerIsAttached() {
        ScriptedMessaging messaging = new ScriptedMessaging();
        ConfigComponentProvider p = provider(messaging);
        CapturingConfigManager manager = new CapturingConfigManager();
        p.attachConfigManager(manager); // what the ConfigManager constructor does post-bootstrap

        JsonObject newConfig = new JsonObject();
        newConfig.addProperty("component", "pushed");
        messaging.simulateMessage(EXPECTED_SET_CONFIG_TOPIC, setConfigPush(newConfig));

        assertEquals(1, manager.applied.size(), "a set-config push on the component inbox must apply");
        assertEquals("pushed", manager.applied.get(0).get("component").getAsString());
    }

    @Test
    void pushedSetConfigBeforeAttachIsDroppedWithoutNPE() {
        // The provider exists BEFORE the ConfigManager (production bootstrap). A push racing
        // ahead of the attach must be dropped, not dereference the null manager.
        ScriptedMessaging messaging = new ScriptedMessaging();
        ConfigComponentProvider p = provider(messaging);

        JsonObject early = new JsonObject();
        assertDoesNotThrow(() ->
                messaging.simulateMessage(EXPECTED_SET_CONFIG_TOPIC, setConfigPush(early)));

        // After the attach, pushes flow normally.
        CapturingConfigManager manager = new CapturingConfigManager();
        p.attachConfigManager(manager);
        JsonObject late = new JsonObject();
        late.addProperty("component", "late");
        messaging.simulateMessage(EXPECTED_SET_CONFIG_TOPIC, setConfigPush(late));
        assertEquals(1, manager.applied.size(), "only the post-attach push applies");
        assertEquals("late", manager.applied.get(0).get("component").getAsString());
    }

    // ----- the 3-attempt retry contract under the framework deadline (§5, slice 1c) -----

    @Test
    void frameworkDeadlineTimeoutsRetryWithFreshRequestsThenFailAfterThree() {
        // Every request's future was settled by the framework deadline (ExecutionException whose
        // cause is TimeoutException). The provider must retry with FRESH requests (the settled
        // future can never complete) and give up after 3 attempts.
        ScriptedMessaging messaging = new ScriptedMessaging();
        messaging.scripted.add(timedOut());
        messaging.scripted.add(timedOut());
        messaging.scripted.add(timedOut());

        RuntimeException ex = assertThrows(RuntimeException.class,
                () -> provider(messaging).loadConfiguration());

        assertTrue(ex.getMessage().contains("3 tries"), "must report the 3-attempt failure: " + ex.getMessage());
        assertEquals(3, messaging.requests.get(), "each retry must issue a fresh request");
    }

    @Test
    void recoversWhenALaterAttemptSucceeds() {
        ScriptedMessaging messaging = new ScriptedMessaging();
        JsonObject cfg = new JsonObject();
        cfg.addProperty("component", "recovered");
        messaging.scripted.add(timedOut());
        messaging.scripted.add(timedOut());
        messaging.scripted.add(replied(cfg));

        JsonObject loaded = provider(messaging).loadConfiguration();

        assertEquals("recovered", loaded.get("component").getAsString());
        assertEquals(3, messaging.requests.get());
    }

    @Test
    void nonTimeoutExecutionFailuresStayFatal() {
        // A non-timeout ExecutionException (e.g. a transport error completing the future) must
        // keep the existing fatal contract — no retry.
        ScriptedMessaging messaging = new ScriptedMessaging();
        ReplyFuture broken = new ReplyFuture("ggcommons/reply-broken");
        broken.trySettle();
        broken.completeExceptionally(new IllegalStateException("transport exploded"));
        messaging.scripted.add(broken);

        RuntimeException ex = assertThrows(RuntimeException.class,
                () -> provider(messaging).loadConfiguration());

        assertEquals(1, messaging.requests.get(), "a non-timeout failure must not be retried");
        assertTrue(ex.getMessage().contains("Failed to load configuration"));
    }
}
