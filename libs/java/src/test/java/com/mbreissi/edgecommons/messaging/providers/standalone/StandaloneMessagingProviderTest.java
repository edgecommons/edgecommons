/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging.providers.standalone;

import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.MessagingConfiguration;
import com.mbreissi.edgecommons.messaging.ReplyFuture;
import com.mbreissi.edgecommons.test.MockConfigurationService;
import com.google.gson.Gson;
import com.google.gson.JsonObject;
import io.moquette.broker.Server;
import io.moquette.broker.config.MemoryConfig;
import org.eclipse.paho.client.mqttv3.MqttClient;
import org.eclipse.paho.client.mqttv3.MqttConnectOptions;
import org.eclipse.paho.client.mqttv3.MqttMessage;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import com.mbreissi.edgecommons.messaging.Qos;

import java.net.ServerSocket;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.Properties;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Direct unit tests for {@link StandaloneMessagingProvider} driven against an in-process Moquette
 * broker (plaintext, anonymous). These cover the local-broker code paths the integration test does
 * not: bounded-queue drop-on-overflow, wildcard subscription routing, JSON-vs-raw payload parsing in
 * {@code messageArrived}, request/reply correlation completion, unsubscribe of an unknown topic, and
 * the {@code requireIotCore} guards on every IoT-Core delegate when no {@code northbound} section is
 * configured.
 *
 * <p>The TLS/cert plumbing ({@code createSslContext}, {@code getSocketFactory}, PEM parsing) is
 * exercised by {@link StandaloneTlsTest}; the IoT-Core happy path needs a TLS broker and is not
 * duplicated here. This class only adds the error branch of {@code getSocketFactory}.
 */
class StandaloneMessagingProviderTest {

    private static Server broker;
    private static int port;
    private static final MockConfigurationService MOCK_CONFIG = new MockConfigurationService();
    private StandaloneMessagingProvider provider;

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
        props.setProperty("data_path", Files.createTempDirectory("moquette-prov").toString() + "/");
        broker = new Server();
        broker.startServer(new MemoryConfig(props));
    }

    @AfterAll
    static void stopBroker() {
        if (broker != null) {
            broker.stopServer();
        }
    }

    @AfterEach
    void closeProvider() {
        if (provider != null) {
            provider.close();
            provider = null;
        }
    }

    /** Builds a local-only (no northbound) provider connected to the in-process broker. */
    private StandaloneMessagingProvider localProvider(String clientId) {
        String json = """
                { "messaging": { "local": { "host": "127.0.0.1", "port": %d, "clientId": "%s" } } }"""
                .formatted(port, clientId);
        MessagingConfiguration cfg = new Gson().fromJson(json, MessagingConfiguration.class);
        return new StandaloneMessagingProvider(cfg, "test-thing");
    }

    private Message msg(String name) {
        JsonObject payload = new JsonObject();
        payload.addProperty("k", "v");
        return MessageBuilder.create(name, "1.0").withPayload(payload).withConfig(MOCK_CONFIG).build();
    }

    @Test
    void connectsAndExposesNativeLocalClientOnly() {
        provider = localProvider("native-clients");
        assertNotNull(provider.getNativeLocalClient());
        // No northbound section -> no northbound client.
        assertNull(provider.getNativeNorthboundClient());
    }

    @Test
    void publishAndSubscribeDeliversParsedJsonMessage() throws Exception {
        provider = localProvider("pubsub");
        String topic = "prov/test/json";
        var latch = new CountDownLatch(1);
        var got = new AtomicReference<Message>();
        var gotTopic = new AtomicReference<String>();
        provider.subscribe(topic, (t, m) -> { gotTopic.set(t); got.set(m); latch.countDown(); }, 1, -1);

        provider.publish(topic, msg("Hello"));

        assertTrue(latch.await(5, TimeUnit.SECONDS), "message not delivered");
        assertEquals("Hello", got.get().getHeader().getName());
        assertEquals(topic, gotTopic.get());
    }

    @Test
    void wildcardSubscriptionRoutesByTopicFilter() throws Exception {
        provider = localProvider("wildcard");
        String filter = "prov/wild/+";
        var latch = new CountDownLatch(1);
        var topicRef = new AtomicReference<String>();
        provider.subscribe(filter, (t, m) -> { topicRef.set(t); latch.countDown(); }, 1, -1);

        provider.publish("prov/wild/leaf", msg("Wild"));

        assertTrue(latch.await(5, TimeUnit.SECONDS), "wildcard message not delivered");
        assertEquals("prov/wild/leaf", topicRef.get());
    }

    @Test
    void rawNonJsonPayloadFallsBackToRawMessage() throws Exception {
        provider = localProvider("rawfallback");
        String topic = "prov/test/raw";
        var latch = new CountDownLatch(1);
        var got = new AtomicReference<Message>();
        provider.subscribe(topic, (t, m) -> { got.set(m); latch.countDown(); }, 1, -1);

        // Publish a non-JSON payload directly via a raw Paho client so messageArrived must use the
        // raw-string fallback branch.
        MqttClient raw = new MqttClient("tcp://127.0.0.1:" + port, "raw-pub");
        MqttConnectOptions opts = new MqttConnectOptions();
        opts.setCleanSession(true);
        raw.connect(opts);
        raw.publish(topic, new MqttMessage("not-json-at-all".getBytes(StandardCharsets.UTF_8)));

        assertTrue(latch.await(5, TimeUnit.SECONDS), "raw message not delivered");
        // Gson parses a bare token leniently, so we only assert a Message was produced.
        assertNotNull(got.get());
        raw.disconnect();
        raw.close();
    }

    @Test
    void boundedQueueDropsOnOverflowWithoutBlocking() throws Exception {
        provider = localProvider("bounded");
        String topic = "prov/bounded";
        // maxConcurrency=1, maxMessages=1: a slow callback + a 1-slot queue forces overflow drops.
        var firstInCallback = new CountDownLatch(1);
        var releaseCallback = new CountDownLatch(1);
        var delivered = new AtomicInteger(0);
        provider.subscribe(topic, (t, m) -> {
            delivered.incrementAndGet();
            firstInCallback.countDown();
            try {
                releaseCallback.await(5, TimeUnit.SECONDS);
            } catch (InterruptedException ignored) {
                Thread.currentThread().interrupt();
            }
        }, 1, 1);

        // First message: enters the callback and blocks there.
        provider.publish(topic, msg("m0"));
        assertTrue(firstInCallback.await(5, TimeUnit.SECONDS), "first callback never ran");

        // Flood while the single callback is blocked. Queue capacity is 1; the rest must be dropped
        // rather than block the MQTT receive thread (the contract under test).
        for (int i = 0; i < 50; i++) {
            provider.publish(topic, msg("m" + i));
        }
        // Give the broker/receiver a moment to process and drop.
        Thread.sleep(300);
        releaseCallback.countDown();

        // The provider must not have delivered all 51 messages (proves drop-on-overflow happened).
        Thread.sleep(300);
        assertTrue(delivered.get() < 51,
                "expected drop-on-overflow but all messages were delivered (" + delivered.get() + ")");
    }

    @Test
    void requestReplyCompletesFutureWithCorrelatedReply() throws Exception {
        provider = localProvider("reqrep");
        String reqTopic = "prov/req";
        // Responder: echoes a reply on the request's replyTo with the same correlation id.
        provider.subscribe(reqTopic, (t, request) -> provider.reply(request, msg("Reply")), 1, -1);

        Message reply = provider.request(reqTopic, msg("Question")).get(5, TimeUnit.SECONDS);
        assertNotNull(reply);
        assertEquals("Reply", reply.getHeader().getName());
    }

    @Test
    void cancelRequestCompletesFutureWithNull() throws Exception {
        provider = localProvider("cancel");
        ReplyFuture future = provider.request("prov/never-answered", msg("Q"));
        provider.cancelRequest(future);
        assertTrue(future.isDone());
        assertNull(future.get(2, TimeUnit.SECONDS));
    }

    @Test
    void unsubscribeUnknownTopicIsNoOp() {
        provider = localProvider("unsub-unknown");
        // Removing a filter that was never subscribed must not throw.
        provider.unsubscribe("prov/never/subscribed");
    }

    @Test
    void publishRawDeliversArbitraryJson() throws Exception {
        provider = localProvider("rawpub");
        String topic = "prov/rawpub";
        var latch = new CountDownLatch(1);
        var got = new AtomicReference<Message>();
        provider.subscribe(topic, (t, m) -> { got.set(m); latch.countDown(); }, 1, -1);

        JsonObject raw = new JsonObject();
        raw.addProperty("answer", 42);
        provider.publishRaw(topic, raw);

        assertTrue(latch.await(5, TimeUnit.SECONDS), "raw publish not delivered");
        assertNotNull(got.get());
    }

    // --- IoT Core API guard branches (no northbound section configured) -----------------------------

    @Test
    void iotCoreDelegatesThrowWhenNotConfigured() {
        provider = localProvider("noiot");
        Message m = msg("X");
        JsonObject raw = new JsonObject();

        assertThrows(IllegalStateException.class,
                () -> provider.publishNorthbound("iot/t", m, Qos.AT_LEAST_ONCE));
        assertThrows(IllegalStateException.class,
                () -> provider.publishNorthboundRaw("iot/t", raw, Qos.AT_LEAST_ONCE));
        assertThrows(IllegalStateException.class,
                () -> provider.subscribeNorthbound("iot/t", (t, msg) -> {}, Qos.AT_MOST_ONCE, 1, -1));
        assertThrows(IllegalStateException.class,
                () -> provider.unsubscribeNorthbound("iot/t"));
        assertThrows(IllegalStateException.class,
                () -> provider.requestNorthbound("iot/t", m));
        assertThrows(IllegalStateException.class,
                () -> provider.replyNorthbound(m, m));
        assertThrows(IllegalStateException.class,
                () -> provider.cancelRequestNorthbound(new ReplyFuture("iot/reply")));
    }

    // --- username/password auth branch in the constructor ----------------------------------------

    @Test
    void connectsWithUsernamePasswordCredentials() throws Exception {
        // A plaintext local config WITH username/password credentials exercises the non-TLS
        // credentials branch in the constructor (setUserName/setPassword). Moquette is running with
        // allow_anonymous=true, so it accepts the connection.
        String json = """
                { "messaging": { "local": {
                    "host": "127.0.0.1", "port": %d, "clientId": "userpass",
                    "credentials": { "username": "alice", "password": "secret" }
                } } }""".formatted(port);
        MessagingConfiguration cfg = new Gson().fromJson(json, MessagingConfiguration.class);
        provider = new StandaloneMessagingProvider(cfg, "test-thing");
        assertNotNull(provider.getNativeLocalClient());

        // Confirm it is actually usable: publish/subscribe round-trip.
        String topic = "prov/userpass";
        var latch = new CountDownLatch(1);
        provider.subscribe(topic, (t, m) -> latch.countDown(), 1, -1);
        provider.publish(topic, msg("Auth"));
        assertTrue(latch.await(5, TimeUnit.SECONDS), "message not delivered with user/pass auth");
    }

    // --- error branches reachable after the client is closed -------------------------------------

    @Test
    void publishOnDisconnectedClientIsSwallowed() throws Exception {
        provider = localProvider("disconnected");
        provider.subscribe("prov/disc/sub", (t, m) -> {}, 1, -1);

        // Disconnect the underlying Paho client (but do not close it): subsequent publishes throw
        // MqttException ("Client is disconnected"), which the provider's internalPublish /
        // publishRaw catch blocks must swallow (no exception escapes to the caller).
        MqttClient native_ = (MqttClient) provider.getNativeLocalClient();
        native_.disconnect();

        assertDoesNotThrow(() -> provider.publish("prov/disc/pub", msg("X")),
                "publish on a disconnected client must be swallowed");
        JsonObject raw = new JsonObject();
        raw.addProperty("x", 1);
        assertDoesNotThrow(() -> provider.publishRaw("prov/disc/raw", raw),
                "publishRaw on a disconnected client must be swallowed");
        // internalUnsubscribe has a broad catch(Exception) -> safe even on a disconnected client.
        assertDoesNotThrow(() -> provider.unsubscribe("prov/disc/sub"),
                "unsubscribe on a disconnected client must be swallowed");
    }

    // --- getSocketFactory error branch ------------------------------------------------------------

    @Test
    void getSocketFactoryReturnsNullOnMissingFiles() {
        // Nonexistent cert/key/CA paths -> the catch block logs and returns null (the guard the
        // provider relies on to refuse a TLS connection when configured certificate files are missing).
        assertNull(StandaloneMessagingProvider.getSocketFactory(
                "/no/such/ca.crt", "/no/such/client.crt", "/no/such/client.key"));
    }
}
