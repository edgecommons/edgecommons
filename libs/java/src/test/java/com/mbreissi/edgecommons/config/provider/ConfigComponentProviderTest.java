/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config.provider;

import com.mbreissi.edgecommons.ParsedCommandLine;
import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.config.ConfigManagerFactory;
import com.mbreissi.edgecommons.config.ConfigurationException;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.ReplyFuture;
import com.mbreissi.edgecommons.test.MockConfigurationService;
import com.mbreissi.edgecommons.test.MockMessagingService;
import com.google.gson.JsonArray;
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

        @Override
        public boolean applyConfigFromProvider(JsonObject rawConfig) {
            applied.add(rawConfig);
            return true;
        }
    }

    private static ReplyFuture timedOut() {
        ReplyFuture f = new ReplyFuture("edgecommons/reply-deadline");
        f.trySettle();
        f.completeExceptionally(new TimeoutException("request timed out (framework deadline)"));
        return f;
    }

    private static ReplyFuture replied(JsonObject body) {
        // A real reply is a full message envelope; loadConfiguration reads its "body".
        JsonObject envelope = new JsonObject();
        envelope.add("body", body);
        ReplyFuture f = new ReplyFuture("edgecommons/reply-ok");
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

    private static ParsedCommandLine configComponentCmdLine() {
        ParsedCommandLine cmdLine = new ParsedCommandLine();
        cmdLine.configArgs = new String[]{"CONFIG_COMPONENT"};
        cmdLine.thingName = "test-thing";
        return cmdLine;
    }

    private static JsonObject json(String json) {
        return com.google.gson.JsonParser.parseString(json).getAsJsonObject();
    }

    private static JsonObject lineage(JsonObject... configs) {
        JsonArray layers = new JsonArray();
        String[] scopeLevels = {"site", "zone", "line", "area"};
        String[] scopeValues = {"dallas", "packaging", "line-7", "north"};
        for (int i = 0; i < configs.length; i++) {
            boolean componentLayer = i == configs.length - 1;
            JsonObject layer = new JsonObject();
            layer.addProperty("id", componentLayer ? "component/Comp" : scopeLevels[i] + "/" + scopeValues[i]);
            layer.addProperty("kind", componentLayer ? "component" : "scope");
            if (componentLayer) {
                layer.addProperty("component", "Comp");
            } else {
                JsonObject scope = new JsonObject();
                scope.addProperty(scopeLevels[i], scopeValues[i]);
                layer.add("scope", scope);
            }
            layer.add("config", configs[i]);
            layers.add(layer);
        }
        JsonObject bundle = new JsonObject();
        bundle.addProperty("lineageVersion", 1);
        bundle.addProperty("catalogVersion", "test");
        bundle.addProperty("component", "Comp");
        bundle.add("layers", layers);
        return bundle;
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
    void lineageReplyMergesLayersIntoEffectiveConfig() throws Exception {
        ScriptedMessaging messaging = new ScriptedMessaging();
        messaging.scripted.add(replied(lineage(json("""
                {"logging":{"level":"INFO"},
                 "identity":{"site":"dallas"},
                 "hierarchy":{"levels":["site","device"]}}"""),
                json("{\"component\":{\"global\":{\"v\":1}}}"))));

        ConfigManager cm = ConfigManagerFactory.create("com.test.Comp",
                configComponentCmdLine(), messaging);
        try {
            assertEquals("INFO", cm.getFullConfig().getAsJsonObject("logging")
                    .get("level").getAsString());
            assertEquals(1, cm.getGlobalConfig().get("v").getAsInt());
        } finally {
            cm.close();
        }
    }

    @Test
    void structuredErrorReplyFailsStartupWithServerCode() {
        ScriptedMessaging messaging = new ScriptedMessaging();
        messaging.scripted.add(replied(json("""
                {"ok":false,
                 "error":{"code":"CONFIG_NOT_FOUND",
                          "message":"No configuration catalog entry for component 'missing'"}}""")));

        ConfigurationException ex = assertThrows(ConfigurationException.class,
                () -> ConfigManagerFactory.create("com.test.Comp",
                        configComponentCmdLine(), messaging));
        assertTrue(ex.getMessage().contains("CONFIG_NOT_FOUND")
                || (ex.getCause() != null && ex.getCause().getMessage().contains("CONFIG_NOT_FOUND")));
    }

    @Test
    void malformedBundleMissingComponentFailsStartup() {
        ScriptedMessaging messaging = new ScriptedMessaging();
        messaging.scripted.add(replied(json("{\"base\":{\"logging\":{\"level\":\"INFO\"}}}")));

        ConfigurationException ex = assertThrows(ConfigurationException.class,
                () -> ConfigManagerFactory.create("com.test.Comp",
                        configComponentCmdLine(), messaging));
        assertTrue(ex.getMessage().contains("LINEAGE_BUNDLE_INVALID")
                || (ex.getCause() != null && ex.getCause().getMessage().contains("LINEAGE_BUNDLE_INVALID")));
    }

    @Test
    void pushedLineageReloadMergesAndAppliesEffectiveConfig() throws Exception {
        ScriptedMessaging messaging = new ScriptedMessaging();
        messaging.scripted.add(replied(lineage(json("{\"component\":{\"global\":{\"v\":1}}}"))));

        ConfigManager cm = ConfigManagerFactory.create("com.test.Comp",
                configComponentCmdLine(), messaging);
        cm.completeInitialization();
        try {
            JsonObject push = lineage(json("{\"logging\":{\"level\":\"WARN\"}}"),
                    json("{\"component\":{\"global\":{\"v\":2}}}"));
            messaging.simulateMessage(EXPECTED_SET_CONFIG_TOPIC, setConfigPush(push));

            assertEquals("WARN", cm.getFullConfig().getAsJsonObject("logging")
                    .get("level").getAsString());
            assertEquals(2, cm.getGlobalConfig().get("v").getAsInt());
        } finally {
            cm.close();
        }
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
        p.start();
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
        p.start();

        JsonObject newConfig = new JsonObject();
        newConfig.addProperty("component", "pushed");
        messaging.simulateMessage(EXPECTED_SET_CONFIG_TOPIC, setConfigPush(newConfig));

        assertEquals(1, manager.applied.size(), "a set-config push on the component inbox must apply");
        assertEquals("pushed", manager.applied.get(0).get("component").getAsString());
    }

    @Test
    void pushedSetConfigBeforeAttachIsDroppedWithoutNPE() {
        // The provider exists BEFORE the ConfigManager (production bootstrap), but its constructor
        // must not subscribe. An early delivery therefore has no callback to race the null manager.
        ScriptedMessaging messaging = new ScriptedMessaging();
        ConfigComponentProvider p = provider(messaging);

        JsonObject early = new JsonObject();
        assertDoesNotThrow(() ->
                messaging.simulateMessage(EXPECTED_SET_CONFIG_TOPIC, setConfigPush(early)));
        assertTrue(messaging.getSubscribedTopics().isEmpty(),
                "provider construction must not start change delivery before manager attachment");

        // After the attach, pushes flow normally.
        CapturingConfigManager manager = new CapturingConfigManager();
        p.attachConfigManager(manager);
        p.start();
        assertEquals(java.util.Set.of(EXPECTED_SET_CONFIG_TOPIC),
                messaging.getSubscribedTopics());
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
        ReplyFuture broken = new ReplyFuture("edgecommons/reply-broken");
        broken.trySettle();
        broken.completeExceptionally(new IllegalStateException("transport exploded"));
        messaging.scripted.add(broken);

        RuntimeException ex = assertThrows(RuntimeException.class,
                () -> provider(messaging).loadConfiguration());

        assertEquals(1, messaging.requests.get(), "a non-timeout failure must not be retried");
        assertTrue(ex.getMessage().contains("Failed to load configuration"));
    }
}
