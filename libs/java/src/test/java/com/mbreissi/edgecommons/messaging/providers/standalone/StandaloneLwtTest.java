/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging.providers.standalone;

import com.mbreissi.edgecommons.messaging.MessagingConfiguration;
import com.google.gson.Gson;
import io.moquette.broker.Server;
import io.moquette.broker.config.MemoryConfig;
import org.eclipse.paho.client.mqttv3.MqttConnectOptions;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.net.ServerSocket;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.Properties;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Tests for the MQTT Last-Will-and-Testament wiring (UNS-CANONICAL-DESIGN §6, D-U9/M7):
 * {@code messaging.lwt} parsing (incl. the schema's numeric {@code qos} enum arriving as
 * {@code 1} or a lossless {@code 1.0}), the connect-options seam
 * ({@link StandaloneMessagingProvider#buildLocalConnectOptions} /
 * {@link StandaloneMessagingProvider#applyLwt}) carrying the will with hard
 * {@code retain=false}, verbatim string vs compact-JSON object payload bytes, and a live
 * local-broker connect with a will registered. NO retain option exists by design.
 */
class StandaloneLwtTest {

    private static Server broker;
    private static int port;

    @BeforeAll
    static void startBroker() throws Exception {
        try (ServerSocket s = new ServerSocket(0)) {
            port = s.getLocalPort();
        }
        Properties props = new Properties();
        props.setProperty("host", "127.0.0.1");
        props.setProperty("port", String.valueOf(port));
        props.setProperty("allow_anonymous", "true");
        props.setProperty("persistence_enabled", "false");
        props.setProperty("data_path", Files.createTempDirectory("moquette-lwt").toString() + "/");
        broker = new Server();
        broker.startServer(new MemoryConfig(props));
    }

    @AfterAll
    static void stopBroker() {
        if (broker != null) {
            broker.stopServer();
        }
    }

    private static MessagingConfiguration cfg(String json) {
        return new Gson().fromJson(json, MessagingConfiguration.class);
    }

    // --- config parsing (Gson) --------------------------------------------------------------------

    @Test
    void parsesLwtSectionWithObjectPayloadAndIntegerQos() {
        MessagingConfiguration c = cfg("""
                { "messaging": { "local": {"host":"h","port":1883,"clientId":"c"},
                  "lwt": { "topic": "ecv1/gw-01/uns-bridge/main/state",
                           "payload": { "status": "UNREACHABLE" }, "qos": 1 } } }""");
        MessagingConfiguration.LwtConfig lwt = c.getMessaging().getLwt();
        assertNotNull(lwt);
        assertEquals("ecv1/gw-01/uns-bridge/main/state", lwt.getTopic());
        assertTrue(lwt.getPayload().isJsonObject());
        assertEquals(1, lwt.getQosOrDefault());
    }

    @Test
    void qosArrivingAsLosslessDoubleParsesCleanly() {
        // The schema types qos as "number" (enum [0,1]); a JSON source can deliver 1.0. Gson's
        // Integer adapter accepts a lossless double — verify the flagged case end-to-end.
        MessagingConfiguration c = cfg("""
                { "messaging": { "local": {"host":"h","port":1883,"clientId":"c"},
                  "lwt": { "topic": "t", "qos": 1.0 } } }""");
        assertEquals(1, c.getMessaging().getLwt().getQosOrDefault());

        MessagingConfiguration c0 = cfg("""
                { "messaging": { "local": {"host":"h","port":1883,"clientId":"c"},
                  "lwt": { "topic": "t", "qos": 0.0 } } }""");
        assertEquals(0, c0.getMessaging().getLwt().getQosOrDefault());
    }

    @Test
    void absentQosDefaultsToOneAndAbsentLwtIsNull() {
        MessagingConfiguration c = cfg("""
                { "messaging": { "local": {"host":"h","port":1883,"clientId":"c"},
                  "lwt": { "topic": "t", "payload": "gone" } } }""");
        assertNull(c.getMessaging().getLwt().getQos());
        assertEquals(1, c.getMessaging().getLwt().getQosOrDefault(), "schema default qos is 1");

        MessagingConfiguration none = cfg("""
                { "messaging": { "local": {"host":"h","port":1883,"clientId":"c"} } }""");
        assertNull(none.getMessaging().getLwt(), "no lwt section -> null (no will registered)");
    }

    // --- the connect-options seam ------------------------------------------------------------------

    @Test
    void buildLocalConnectOptionsCarriesTheWillWithRetainFalse() throws Exception {
        MessagingConfiguration c = cfg("""
                { "messaging": { "local": {"host":"h","port":1883,"clientId":"c"},
                  "lwt": { "topic": "ecv1/gw-01/bridge/main/state",
                           "payload": { "status": "UNREACHABLE" }, "qos": 1 } } }""");
        MqttConnectOptions options = StandaloneMessagingProvider.buildLocalConnectOptions(
                c.getMessaging().getLocal(), c.getMessaging().getLwt());

        assertEquals("ecv1/gw-01/bridge/main/state", options.getWillDestination());
        assertNotNull(options.getWillMessage());
        assertEquals(1, options.getWillMessage().getQos());
        assertFalse(options.getWillMessage().isRetained(), "retain must be hard false (D-U9: no retain)");
        // Object payload -> compact JSON bytes.
        assertEquals("{\"status\":\"UNREACHABLE\"}",
                new String(options.getWillMessage().getPayload(), StandardCharsets.UTF_8));
        assertTrue(options.isAutomaticReconnect(), "auto-reconnect re-registers the will on reconnect");
    }

    @Test
    void stringPayloadIsPublishedVerbatimAsUtf8Bytes() throws Exception {
        MessagingConfiguration c = cfg("""
                { "messaging": { "local": {"host":"h","port":1883,"clientId":"c"},
                  "lwt": { "topic": "t/will", "payload": "OFFLINE \\u2620", "qos": 0 } } }""");
        MqttConnectOptions options = StandaloneMessagingProvider.buildLocalConnectOptions(
                c.getMessaging().getLocal(), c.getMessaging().getLwt());

        assertArrayEquals("OFFLINE ☠".getBytes(StandardCharsets.UTF_8),
                options.getWillMessage().getPayload(), "a string payload is published VERBATIM (no JSON quoting)");
        assertEquals(0, options.getWillMessage().getQos());
        assertFalse(options.getWillMessage().isRetained());
    }

    @Test
    void absentPayloadYieldsEmptyWillBytes() throws Exception {
        MqttConnectOptions options = new MqttConnectOptions();
        MessagingConfiguration c = cfg("""
                { "messaging": { "local": {"host":"h","port":1883,"clientId":"c"},
                  "lwt": { "topic": "t/will" } } }""");
        StandaloneMessagingProvider.applyLwt(options, c.getMessaging().getLwt());
        assertEquals(0, options.getWillMessage().getPayload().length);
        assertEquals(1, options.getWillMessage().getQos(), "absent qos defaults to 1");
    }

    @Test
    void noLwtSectionRegistersNoWill() throws Exception {
        MessagingConfiguration c = cfg("""
                { "messaging": { "local": {"host":"h","port":1883,"clientId":"c"} } }""");
        MqttConnectOptions options = StandaloneMessagingProvider.buildLocalConnectOptions(
                c.getMessaging().getLocal(), c.getMessaging().getLwt());
        assertNull(options.getWillDestination());
        assertNull(options.getWillMessage());
    }

    @Test
    void invalidQosOrMissingTopicFailFast() {
        MqttConnectOptions options = new MqttConnectOptions();
        MessagingConfiguration badQos = cfg("""
                { "messaging": { "local": {"host":"h","port":1883,"clientId":"c"},
                  "lwt": { "topic": "t", "qos": 2 } } }""");
        IllegalArgumentException qosEx = assertThrows(IllegalArgumentException.class,
                () -> StandaloneMessagingProvider.applyLwt(options, badQos.getMessaging().getLwt()));
        assertTrue(qosEx.getMessage().contains("qos"));

        MessagingConfiguration noTopic = cfg("""
                { "messaging": { "local": {"host":"h","port":1883,"clientId":"c"},
                  "lwt": { "payload": "x" } } }""");
        IllegalArgumentException topicEx = assertThrows(IllegalArgumentException.class,
                () -> StandaloneMessagingProvider.applyLwt(options, noTopic.getMessaging().getLwt()));
        assertTrue(topicEx.getMessage().contains("topic"));
    }

    // --- live connect with a will ------------------------------------------------------------------

    @Test
    void providerWithLwtConnectsAndOperatesNormally() throws Exception {
        // The will travels in the CONNECT packet: the broker accepting the connection (and normal
        // pub/sub continuing to work) proves the registered will is well-formed. The will firing
        // requires an ungraceful socket drop, which Paho's public API cannot produce — the payload/
        // qos/retain contract is asserted against the connect options above.
        String json = """
                { "messaging": { "local": { "host": "127.0.0.1", "port": %d, "clientId": "lwt-live" },
                  "lwt": { "topic": "lwt/live/state", "payload": { "status": "UNREACHABLE" }, "qos": 1 } } }"""
                .formatted(port);
        StandaloneMessagingProvider provider =
                new StandaloneMessagingProvider(cfg(json), "test-thing");
        try {
            assertTrue(provider.connected());
            CountDownLatch latch = new CountDownLatch(1);
            provider.subscribe("lwt/live/echo", (t, m) -> latch.countDown(), 1, -1);
            provider.publishRaw("lwt/live/echo", new com.google.gson.JsonObject());
            assertTrue(latch.await(5, TimeUnit.SECONDS), "pub/sub must work with a will registered");
        } finally {
            provider.close();
        }
    }
}
