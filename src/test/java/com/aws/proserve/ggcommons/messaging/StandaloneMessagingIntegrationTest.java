/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging;

import com.aws.proserve.ggcommons.messaging.providers.standalone.StandaloneMessagingProvider;
import com.aws.proserve.ggcommons.test.MockConfigurationService;
import com.google.gson.Gson;
import com.google.gson.JsonObject;
import io.moquette.broker.Server;
import io.moquette.broker.config.MemoryConfig;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.net.ServerSocket;
import java.nio.file.Files;
import java.util.Properties;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

import software.amazon.awssdk.aws.greengrass.model.QOS;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Full standalone-mode integration tests for {@link StandaloneMessagingProvider} against an
 * in-process Moquette MQTT broker (no external broker, no AWS). Exercises connect, publish,
 * subscribe (incl. wildcards and maxConcurrency), request/reply, raw publish, unsubscribe,
 * cancel, and close over a real MQTT transport.
 */
class StandaloneMessagingIntegrationTest {

    private static Server broker;
    private static int port;
    private static StandaloneMessagingProvider provider;
    private static final MockConfigurationService CFG = new MockConfigurationService();

    @BeforeAll
    static void startBrokerAndProvider() throws Exception {
        try (ServerSocket s = new ServerSocket(0)) {
            port = s.getLocalPort();
        }
        Properties props = new Properties();
        props.setProperty("host", "127.0.0.1");
        props.setProperty("port", String.valueOf(port));
        props.setProperty("allow_anonymous", "true");
        props.setProperty("persistence_enabled", "false");
        props.setProperty("data_path", Files.createTempDirectory("moquette").toString() + "/");
        broker = new Server();
        broker.startServer(new MemoryConfig(props));

        String json = "{ \"messaging\": { \"local\": {" +
                "\"host\": \"127.0.0.1\", \"port\": " + port + ", \"clientId\": \"itest-local\" } } }";
        MessagingConfiguration config = new Gson().fromJson(json, MessagingConfiguration.class);
        provider = new StandaloneMessagingProvider(config, "test-thing");
    }

    @AfterAll
    static void stop() {
        if (provider != null) {
            provider.close();
        }
        if (broker != null) {
            broker.stopServer();
        }
    }

    private Message msg(String name, String key, String value) {
        JsonObject payload = new JsonObject();
        payload.addProperty(key, value);
        return MessageBuilder.create(name, "1.0").withPayload(payload).withConfig(CFG).build();
    }

    @Test
    void publishAndSubscribeLocal() throws Exception {
        String topic = "itest/pubsub";
        CountDownLatch latch = new CountDownLatch(1);
        AtomicReference<Message> received = new AtomicReference<>();
        provider.subscribe(topic, (t, m) -> { received.set(m); latch.countDown(); }, 1);

        provider.publish(topic, msg("Hello", "k", "v"));

        assertTrue(latch.await(5, TimeUnit.SECONDS), "message should be delivered");
        assertNotNull(received.get());
        assertEquals("Hello", received.get().getHeader().getName());
        provider.unsubscribe(topic);
    }

    @Test
    void subscribeWithWildcard() throws Exception {
        CountDownLatch latch = new CountDownLatch(1);
        provider.subscribe("itest/wild/+", (t, m) -> latch.countDown(), 1);
        provider.publish("itest/wild/abc", msg("W", "k", "v"));
        assertTrue(latch.await(5, TimeUnit.SECONDS), "wildcard subscription should match");
        provider.unsubscribe("itest/wild/+");
    }

    @Test
    void rawPublishDeliversNonEnvelopePayload() throws Exception {
        String topic = "itest/raw";
        CountDownLatch latch = new CountDownLatch(1);
        AtomicReference<Message> received = new AtomicReference<>();
        provider.subscribe(topic, (t, m) -> { received.set(m); latch.countDown(); }, 1);

        JsonObject raw = new JsonObject();
        raw.addProperty("just", "data");
        provider.publishRaw(topic, raw);

        assertTrue(latch.await(5, TimeUnit.SECONDS));
        assertNotNull(received.get().getRaw());
        provider.unsubscribe(topic);
    }

    @Test
    void requestReplyOverBroker() throws Exception {
        String reqTopic = "itest/request";
        provider.subscribe(reqTopic, (t, request) -> {
            Message reply = msg("ReplyMsg", "answer", "42");
            provider.reply(request, reply);
        }, 1);

        Message request = msg("RequestMsg", "q", "question");
        String correlationId = request.getCorrelationId();
        CompletableFuture<Message> future = provider.request(reqTopic, request);
        Message reply = future.get(5, TimeUnit.SECONDS);

        assertNotNull(reply);
        assertEquals("ReplyMsg", reply.getHeader().getName());
        assertEquals(correlationId, reply.getCorrelationId());
        provider.unsubscribe(reqTopic);
    }

    @Test
    void cancelRequestCompletesFutureWithNull() throws Exception {
        Message request = msg("CancelReq", "q", "x");
        ReplyFuture future = provider.request("itest/never-answered", request);
        provider.cancelRequest(future);
        assertTrue(future.isDone());
        assertEquals(null, future.get(2, TimeUnit.SECONDS));
    }

    @Test
    void iotCoreMethodsThrowWhenNotConfigured() {
        // The integration config has no iotCore section, so IoT Core operations must fail clearly.
        Message m = msg("X", "k", "v");
        assertThrows(IllegalStateException.class, () -> provider.publishToIoTCore("itest/iot", m, QOS.AT_LEAST_ONCE));
        assertThrows(IllegalStateException.class,
                () -> provider.publishToIoTCoreRaw("itest/iot", new JsonObject(), QOS.AT_LEAST_ONCE));
        assertThrows(IllegalStateException.class,
                () -> provider.subscribeToIoTCore("itest/iot", (t, msg) -> { }, QOS.AT_LEAST_ONCE, 1));
        assertThrows(IllegalStateException.class, () -> provider.unsubscribeFromIoTCore("itest/iot"));
        assertThrows(IllegalStateException.class, () -> provider.requestFromIoTCore("itest/iot", m));
    }

    @Test
    void nativeClientsReflectConfiguration() {
        assertNotNull(provider.getNativeLocalClient());
        assertNull(provider.getNativeIotCoreClient(), "iotCore client is null when not configured");
    }
}
