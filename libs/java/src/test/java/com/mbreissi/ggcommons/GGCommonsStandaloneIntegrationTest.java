/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons;

import com.mbreissi.ggcommons.messaging.Message;
import com.mbreissi.ggcommons.messaging.MessageBuilder;
import com.mbreissi.ggcommons.messaging.MessagingClient;
import com.mbreissi.ggcommons.messaging.ReplyFuture;
import com.mbreissi.ggcommons.metrics.Metric;
import com.mbreissi.ggcommons.metrics.MetricBuilder;
import com.google.gson.JsonObject;
import io.moquette.broker.Server;
import io.moquette.broker.config.MemoryConfig;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.File;
import java.net.ServerSocket;
import java.nio.file.Files;
import java.util.HashMap;
import java.util.Map;
import java.util.Properties;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

import software.amazon.awssdk.aws.greengrass.model.QOS;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * End-to-end STANDALONE bring-up of {@link GGCommons} against an in-process Moquette broker.
 * Covers GGCommons.init in STANDALONE mode wiring the config manager, messaging client (dual
 * MQTT, IoT Core omitted), metric emitter and heartbeat — then exercises messaging and metrics
 * through the public accessors and shuts down cleanly.
 */
class GGCommonsStandaloneIntegrationTest {

    private static Server broker;
    private static int port;
    private static GGCommons gg;
    private static File workDir;

    @BeforeAll
    static void setup() throws Exception {
        try (ServerSocket s = new ServerSocket(0)) {
            port = s.getLocalPort();
        }
        Properties props = new Properties();
        props.setProperty("host", "127.0.0.1");
        props.setProperty("port", String.valueOf(port));
        props.setProperty("allow_anonymous", "true");
        props.setProperty("persistence_enabled", "false");
        props.setProperty("data_path", Files.createTempDirectory("moquette-gg").toString() + "/");
        broker = new Server();
        broker.startServer(new MemoryConfig(props));

        workDir = Files.createTempDirectory("ggstandalone").toFile();

        File msgCfg = new File(workDir, "standalone-messaging.json");
        Files.write(msgCfg.toPath(), """
                { "messaging": { "local": {"host": "127.0.0.1", "port": %s, "clientId": "gg-local" } } }"""
                .formatted(port).getBytes());

        // A distinct clientId for the deprecated-constructor bring-up below. It connects to the same
        // broker while this class's primary `gg` client is still live; reusing "gg-local" would put
        // two clients on one clientId, and with automaticReconnect(true) the broker's duplicate-id
        // takeover triggers a reconnect war that can fail the second connect under CI timing.
        File legacyMsgCfg = new File(workDir, "standalone-messaging-legacy.json");
        Files.write(legacyMsgCfg.toPath(), """
                { "messaging": { "local": {"host": "127.0.0.1", "port": %s, "clientId": "gg-legacy" } } }"""
                .formatted(port).getBytes());

        File appCfg = new File(workDir, "config.json");
        String metricLog = new File(workDir, "metric.log").getAbsolutePath().replace("\\", "/");
        Files.write(appCfg.toPath(), """
                {\
                "logging": {"level": "INFO"},\
                "metricEmission": {"target": "log", "targetConfig": {"logFileName": "%s"}},\
                "heartbeat": {"intervalSecs": 3600, "measures": {"cpu": true, "memory": true},\
                  "targets": [{"type":"metric"},{"type":"messaging","config":{"topic":"heartbeat/test","destination":"ipc"}}]},\
                "tags": {"env": "test"},\
                "component": {"global": {"setting": "value"}}\
                }"""
                .formatted(metricLog).getBytes());

        String[] args = {
                "-t", "test-thing",
                "--platform", "HOST", "--transport", "MQTT", msgCfg.getAbsolutePath(),
                "-c", "FILE", appCfg.getAbsolutePath()
        };
        gg = GGCommonsBuilder.create("com.test.StandaloneComponent").withArgs(args).build();
    }

    @AfterAll
    static void teardown() {
        if (gg != null) {
            gg.shutdown();
        }
        if (broker != null) {
            broker.stopServer();
        }
    }

    @Test
    void bringsUpAllSubsystems() {
        assertNotNull(gg.getConfigManager());
        assertNotNull(gg.getMessaging());
        assertNotNull(gg.getMetrics());
        assertEquals("test-thing", gg.getConfigManager().getThingName());
        assertEquals("StandaloneComponent", gg.getConfigManager().getComponentName());
    }

    @Test
    void messagingClientPublishSubscribeRoundTrip() throws Exception {
        MessagingClient mc = gg.getMessaging();
        String topic = "gg/itest/topic";
        var latch = new CountDownLatch(1);
        var received = new AtomicReference<Message>();
        mc.subscribe(topic, (t, m) -> { received.set(m); latch.countDown(); }, 1);

        var payload = new JsonObject();
        payload.addProperty("hello", "world");
        Message m = MessageBuilder.create("Greeting", "1.0")
                .withPayload(payload).withConfig(gg.getConfigManager()).build();
        mc.publish(topic, m);

        assertTrue(latch.await(5, TimeUnit.SECONDS));
        assertEquals("Greeting", received.get().getHeader().getName());
        mc.unsubscribe(topic);
    }

    @Test
    void metricEmitterDefinesAndEmits() {
        Metric metric = MetricBuilder.create("itest_metric")
                .addMeasure("count", "Count", 1)
                .withConfig(gg.getConfigManager())
                .build();
        gg.getMetrics().defineMetric(metric);
        assertTrue(gg.getMetrics().isMetricDefined("itest_metric"));

        var values = new HashMap<String, Float>();
        values.put("count", 3.0f);
        assertDoesNotThrow(() -> gg.getMetrics().emitMetricNow("itest_metric", values));
    }

    @Test
    void messagingClientDelegationAndIotCoreGuards() throws Exception {
        MessagingClient mc = gg.getMessaging();

        String reqTopic = "gg/itest/req";
        mc.subscribe(reqTopic, (t, request) -> {
            var rp = new JsonObject();
            rp.addProperty("ok", "yes");
            Message reply = MessageBuilder.create("R", "1.0")
                    .withPayload(rp).withConfig(gg.getConfigManager()).build();
            mc.reply(request, reply);
        }, 1);
        var qp = new JsonObject();
        qp.addProperty("q", "x");
        Message req = MessageBuilder.create("Q", "1.0").withPayload(qp).withConfig(gg.getConfigManager()).build();
        Message reply = mc.request(reqTopic, req).get(5, TimeUnit.SECONDS);
        assertEquals(req.getCorrelationId(), reply.getCorrelationId());
        mc.unsubscribe(reqTopic);

        var raw = new JsonObject();
        raw.addProperty("r", "1");
        assertDoesNotThrow(() -> mc.publishRaw("gg/itest/raw", raw));
        assertNotNull(mc.getNativeLocalClient());
        assertTrue(MessagingClient.topicMatchesFilter("a/+", "a/b"));

        // IoT Core is not configured in this standalone config -> delegation must throw.
        Message m = MessageBuilder.create("X", "1.0").withPayload(raw).withConfig(gg.getConfigManager()).build();
        assertThrows(IllegalStateException.class, () -> mc.publishToIoTCore("iot/t", m, QOS.AT_LEAST_ONCE));
        assertThrows(IllegalStateException.class, () -> mc.subscribeToIoTCore("iot/t", (t, msg) -> {}, QOS.AT_LEAST_ONCE));
        assertThrows(IllegalStateException.class, () -> mc.unsubscribeFromIoTCore("iot/t"));
        assertThrows(IllegalStateException.class, () -> mc.publishToIoTCoreRaw("iot/t", raw, QOS.AT_LEAST_ONCE));
        assertThrows(IllegalStateException.class, () -> mc.replyToIoTCore(m, m));
        assertThrows(IllegalStateException.class, () -> mc.cancelRequestFromIoTCore(new ReplyFuture("x")));

        // local cancel of an unanswered request
        Message req2 = MessageBuilder.create("Q2", "1.0").withPayload(raw).withConfig(gg.getConfigManager()).build();
        ReplyFuture pending = mc.request("gg/itest/unanswered", req2);
        mc.cancelRequest(pending);
        assertTrue(pending.isDone());
    }

    @Test
    void deprecatedConstructorBringsUpStandalone() {
        String[] args = {
                "-t", "legacy-thing",
                "--platform", "HOST", "--transport", "MQTT",
                new File(workDir, "standalone-messaging-legacy.json").getAbsolutePath(),
                "-c", "FILE", new File(workDir, "config.json").getAbsolutePath()
        };
        GGCommons legacy = new GGCommons("com.test.LegacyComponent", args);
        try {
            assertNotNull(legacy.getMessaging());
            assertEquals("legacy-thing", legacy.getConfigManager().getThingName());
        } finally {
            legacy.shutdown();
        }
    }
}
