/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

import com.mbreissi.edgecommons.test.MockConfigurationService;
import com.mbreissi.edgecommons.test.MockMessagingService;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.*;

/**
 * The library-owned {@code cfg} publisher (UNS-CANONICAL-DESIGN §4.3): publishes the effective
 * (redacted) config to {@code ecv1/{device}/{component}/main/cfg} through the privileged seam —
 * at startup ({@code publishNow}) and on every configuration change — with redaction v1:
 * {@code $secret} refs stay unresolved, {@code messaging.*.credentials} values and any
 * {@code password}/{@code pin} key become {@code "***"}.
 */
class EffectiveConfigPublisherTest {

    /** The default mock identity's UNS cfg topic (device=test-thing, component=TestComponent). */
    private static final String CFG_TOPIC = "ecv1/test-thing/TestComponent/main/cfg";

    private static JsonObject obj(String json) {
        return JsonParser.parseString(json).getAsJsonObject();
    }

    private static MockConfigurationService configWith(String fullConfigJson) {
        MockConfigurationService config = new MockConfigurationService();
        config.setFullConfig(obj(fullConfigJson));
        return config;
    }

    @Test
    void publishNowPublishesTheRedactedConfigOnTheUnsCfgTopic() {
        MockConfigurationService config = configWith("""
                {"component":{"global":{}},"logging":{"level":"INFO"}}""");
        MockMessagingService messaging = new MockMessagingService();

        new EffectiveConfigPublisher(config, messaging).publishNow();

        List<MockMessagingService.PublishedMessage> published = messaging.getPublishedMessages();
        assertEquals(1, published.size());
        assertEquals(CFG_TOPIC, published.get(0).topic);
        assertTrue(published.get(0).reserved,
                "cfg publishes must go through the privileged ReservedPublisher seam");
        assertEquals("cfg", published.get(0).message.getHeader().getName());

        JsonObject body = published.get(0).message.toDict().getAsJsonObject("body");
        assertTrue(body.has("config"), "the body is {\"config\": <effective config, redacted>}");
        assertEquals("INFO", body.getAsJsonObject("config")
                .getAsJsonObject("logging").get("level").getAsString());
    }

    @Test
    void republishesOnConfigurationChange() {
        MockConfigurationService config = configWith("""
                {"component":{"global":{}}}""");
        MockMessagingService messaging = new MockMessagingService();

        EffectiveConfigPublisher publisher = new EffectiveConfigPublisher(config, messaging);
        publisher.publishNow();
        assertEquals(1, messaging.getPublishedMessages().size());

        // The publisher registered itself as a configuration-change listener.
        config.simulateConfigurationChange();
        assertEquals(2, messaging.getPublishedMessages().size(),
                "each configuration change must republish the effective config");
        assertEquals(CFG_TOPIC, messaging.getPublishedMessages().get(1).topic);
    }

    @Test
    void missingIdentityIsASafeNoOp() {
        MockConfigurationService config = configWith("""
                {"component":{"global":{}}}""");
        config.setComponentIdentity(null); // the test/subclass bring-up case
        MockMessagingService messaging = new MockMessagingService();

        assertDoesNotThrow(() -> new EffectiveConfigPublisher(config, messaging).publishNow());
        assertTrue(messaging.getPublishedMessages().isEmpty());
    }

    @Test
    void redactedEffectiveConfigIsTheSharedSnapshotSource() {
        // The get-configuration verb (DESIGN-uns §9.5 Flow B) and the cfg push must agree: the
        // accessor returns the SAME redacted snapshot the publisher publishes.
        MockConfigurationService config = configWith("""
                {"component":{"global":{}},"messaging":{"local":{"credentials":"secret"}}}""");
        MockMessagingService messaging = new MockMessagingService();
        EffectiveConfigPublisher publisher = new EffectiveConfigPublisher(config, messaging);

        JsonObject snapshot = publisher.redactedEffectiveConfig();
        assertNotNull(snapshot);
        assertEquals("***", snapshot.getAsJsonObject("messaging")
                .getAsJsonObject("local").get("credentials").getAsString(),
                "the snapshot is redacted (redaction v1)");
        assertEquals("secret", config.getFullConfig().getAsJsonObject("messaging")
                .getAsJsonObject("local").get("credentials").getAsString(),
                "redaction works on a deep copy - the live config is not mutated");

        publisher.publishNow();
        assertEquals(snapshot, messaging.getPublishedMessages().get(0).message.toDict()
                .getAsJsonObject("body").getAsJsonObject("config"),
                "the cfg push body and the accessor snapshot are identical");
    }

    @Test
    void redactedEffectiveConfigIsNullWithoutAConfig() {
        MockConfigurationService config = new MockConfigurationService() {
            @Override
            public JsonObject getFullConfig() {
                return null;
            }
        };
        assertNull(new EffectiveConfigPublisher(config, new MockMessagingService())
                .redactedEffectiveConfig());
    }

    // ----- redaction v1 -----

    @Test
    void redactsMessagingCredentialsAtAnyDepthUnderMessaging() {
        JsonObject redacted = EffectiveConfigPublisher.redact(obj("""
                {"messaging":{
                   "local":{"credentials":{"username":"u","password":"p"},"host":"h"},
                   "northbound":{"credentials":"inline-string"},
                   "requestTimeoutSeconds":30}}"""));

        JsonObject messaging = redacted.getAsJsonObject("messaging");
        assertEquals("***", messaging.getAsJsonObject("local").get("credentials").getAsString(),
                "messaging.*.credentials must be replaced wholesale with ***");
        assertEquals("***", messaging.getAsJsonObject("northbound").get("credentials").getAsString());
        assertEquals("h", messaging.getAsJsonObject("local").get("host").getAsString());
        assertEquals(30, messaging.get("requestTimeoutSeconds").getAsInt());
    }

    @Test
    void credentialsKeyOutsideMessagingIsNotRedactedWholesale() {
        // The rule is messaging.*.credentials; the top-level credentials (vault) section keeps its
        // structure (its sensitive leaves are $secret refs / files, not inline material).
        JsonObject redacted = EffectiveConfigPublisher.redact(obj("""
                {"credentials":{"vault":{"path":"/v"}},
                 "nested":{"messaging":{"x":{"credentials":"keep"}}}}"""));
        assertEquals("/v", redacted.getAsJsonObject("credentials")
                .getAsJsonObject("vault").get("path").getAsString());
        // A NESTED messaging key (not the top-level section) does not trigger the rule.
        assertEquals("keep", redacted.getAsJsonObject("nested").getAsJsonObject("messaging")
                .getAsJsonObject("x").get("credentials").getAsString());
    }

    @Test
    void redactsPasswordAndPinKeysAnywhereCaseInsensitively() {
        JsonObject redacted = EffectiveConfigPublisher.redact(obj("""
                {"a":{"password":"hunter2","Pin":"1234","deep":{"PASSWORD":{"structured":"x"}}},
                 "list":[{"pin":"0000","ok":"v"}],
                 "password":"top"}"""));

        JsonObject a = redacted.getAsJsonObject("a");
        assertEquals("***", a.get("password").getAsString());
        assertEquals("***", a.get("Pin").getAsString());
        assertEquals("***", a.getAsJsonObject("deep").get("PASSWORD").getAsString(),
                "a structured password value is replaced wholesale");
        assertEquals("***", redacted.getAsJsonArray("list").get(0)
                .getAsJsonObject().get("pin").getAsString());
        assertEquals("v", redacted.getAsJsonArray("list").get(0)
                .getAsJsonObject().get("ok").getAsString());
        assertEquals("***", redacted.get("password").getAsString());
    }

    @Test
    void secretRefsStayUnresolved() {
        JsonObject redacted = EffectiveConfigPublisher.redact(obj("""
                {"streaming":{"kinesis":{"apiKey":{"$secret":"kinesis-key"}}}}"""));
        // The ref is published verbatim - never resolved, so no secret material can leak.
        assertEquals("kinesis-key", redacted.getAsJsonObject("streaming")
                .getAsJsonObject("kinesis").getAsJsonObject("apiKey").get("$secret").getAsString());
    }

    @Test
    void redactionDoesNotMutateTheSourceConfig() {
        JsonObject source = obj("""
                {"a":{"password":"hunter2"}}""");
        EffectiveConfigPublisher.redact(source);
        assertEquals("hunter2", source.getAsJsonObject("a").get("password").getAsString(),
                "redact() must operate on a deep copy");
    }

    @Test
    void publishFailuresAreSwallowed() {
        // A messaging failure must never crash the component (best-effort).
        MockConfigurationService config = configWith("""
                {"component":{"global":{}}}""");
        MockMessagingService messaging = new MockMessagingService() {
            @Override
            protected void publishReserved(String topic, com.mbreissi.edgecommons.messaging.Message m) {
                throw new RuntimeException("broker down");
            }
        };
        EffectiveConfigPublisher publisher = new EffectiveConfigPublisher(config, messaging);
        assertDoesNotThrow(publisher::publishNow);
        assertTrue(publisher.onConfigurationChanged(), "the listener contract still returns true");
    }
}
