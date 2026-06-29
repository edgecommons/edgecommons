/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.messaging;

import com.mbreissi.ggcommons.messaging.providers.standalone.StandaloneMessagingProvider;
import com.mbreissi.ggcommons.test.MockConfigurationService;
import com.google.gson.Gson;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.io.File;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assumptions.assumeTrue;

/**
 * Combined STANDALONE dual-broker integration test: a single
 * {@link StandaloneMessagingProvider} connected to BOTH a local broker AND IoT Core at the
 * same time, exercising both transports.
 *
 * <p>To run without real AWS, the {@code iotCore} endpoint is pointed at the SAME shared EMQX
 * as {@code local} but over the mutual-TLS listener (:8883) — so the real dual-client /
 * dual-transport code path runs end-to-end. Because both point at one broker, this validates
 * that both connections are live and both method sets work; distinct topics are used per
 * transport (true cross-broker isolation would need two separate brokers).
 *
 * <p>Self-skips (JUnit assumptions) unless {@code GGCOMMONS_TLS_CERTS_DIR} points at a certs
 * directory with {@code ca.crt}/{@code client.crt}/{@code client.key} AND the broker is
 * reachable on :1883 (plaintext) and :8883 (mutual TLS) — so a normal {@code mvn verify}
 * stays green.
 */
class StandaloneDualBrokerIntegrationTest {

    private static final MockConfigurationService CFG = new MockConfigurationService();

    private static String certsDir() {
        return System.getenv("GGCOMMONS_TLS_CERTS_DIR");
    }

    private static boolean haveCerts(String dir) {
        return dir != null && !dir.isEmpty()
                && new File(dir, "ca.crt").exists()
                && new File(dir, "client.crt").exists()
                && new File(dir, "client.key").exists();
    }

    private static String dualJson(String dir) {
        String ca = (dir + "/ca.crt").replace("\\", "/");
        String cert = (dir + "/client.crt").replace("\\", "/");
        String key = (dir + "/client.key").replace("\\", "/");
        return """
                { "messaging": {
                  "local": {"host": "localhost", "port": 1883, "clientId": "ggcommons-java-dual-local"},
                  "iotCore": {"endpoint": "localhost", "port": 8883, "clientId": "ggcommons-java-dual-iot",
                    "credentials": {"caPath": "%s", "certPath": "%s", "keyPath": "%s"}}
                } }"""
                .formatted(ca, cert, key);
    }

    private static StandaloneMessagingProvider connectOrSkip() {
        String dir = certsDir();
        assumeTrue(haveCerts(dir), "set GGCOMMONS_TLS_CERTS_DIR to a dir with ca.crt/client.crt/client.key");
        MessagingConfiguration config =
                new Gson().fromJson(dualJson(dir), MessagingConfiguration.class);
        // The config must carry BOTH brokers.
        assertNotNull(config.getMessaging().getLocal(), "local broker config");
        assertNotNull(config.getMessaging().getIotCore(), "iotCore broker config");
        try {
            return new StandaloneMessagingProvider(config, "ggcommons-java-dual-it");
        } catch (Exception e) {
            assumeTrue(false, "dual broker not reachable (local :1883 + TLS :8883): " + e.getMessage());
            return null; // unreachable
        }
    }

    private Message msg(String name, String key, String value) {
        var payload = new JsonObject();
        payload.addProperty(key, value);
        return MessageBuilder.create(name, "1.0").withPayload(payload).withConfig(CFG).build();
    }

    @Test
    void bothTransportsDeliverSimultaneously() throws Exception {
        StandaloneMessagingProvider provider = connectOrSkip();
        try {
            // Both native clients exist when both sections are configured.
            assertNotNull(provider.getNativeLocalClient(), "local client");
            assertNotNull(provider.getNativeIotCoreClient(), "iotCore client");

            String localTopic = "ggcommons/dual/java/local/" + System.nanoTime();
            String iotTopic = "ggcommons/dual/java/iot/" + System.nanoTime();
            var localLatch = new CountDownLatch(1);
            var iotLatch = new CountDownLatch(1);
            var localMsg = new AtomicReference<Message>();
            var iotMsg = new AtomicReference<Message>();

            provider.subscribe(localTopic, (t, m) -> { localMsg.set(m); localLatch.countDown(); }, 1);
            provider.subscribeToIoTCore(iotTopic, (t, m) -> { iotMsg.set(m); iotLatch.countDown(); },
                    QOS.AT_LEAST_ONCE, 1);

            provider.publish(localTopic, msg("LocalMsg", "via", "local"));
            provider.publishToIoTCore(iotTopic, msg("IotMsg", "via", "iot"), QOS.AT_LEAST_ONCE);

            assertTrue(localLatch.await(5, TimeUnit.SECONDS), "local transport should deliver");
            assertTrue(iotLatch.await(5, TimeUnit.SECONDS), "IoT Core transport should deliver");
            assertEquals("LocalMsg", localMsg.get().getHeader().getName());
            assertEquals("IotMsg", iotMsg.get().getHeader().getName());

            provider.unsubscribe(localTopic);
            provider.unsubscribeFromIoTCore(iotTopic);
        } finally {
            provider.close();
        }
    }

    @Test
    void requestReplyOnBothTransports() throws Exception {
        StandaloneMessagingProvider provider = connectOrSkip();
        try {
            String localReq = "ggcommons/dual/java/local/req/" + System.nanoTime();
            provider.subscribe(localReq, (t, req) -> provider.reply(req, msg("LReply", "answer", "local")), 1);
            Message localRequest = msg("LReq", "q", "1");
            String localCid = localRequest.getCorrelationId();
            Message localReply = provider.request(localReq, localRequest).get(5, TimeUnit.SECONDS);
            assertNotNull(localReply);
            assertEquals("LReply", localReply.getHeader().getName());
            assertEquals(localCid, localReply.getCorrelationId());

            String iotReq = "ggcommons/dual/java/iot/req/" + System.nanoTime();
            provider.subscribeToIoTCore(iotReq,
                    (t, req) -> provider.replyToIoTCore(req, msg("IReply", "answer", "iot")),
                    QOS.AT_LEAST_ONCE, 1);
            Message iotRequest = msg("IReq", "q", "2");
            String iotCid = iotRequest.getCorrelationId();
            Message iotReply = provider.requestFromIoTCore(iotReq, iotRequest).get(5, TimeUnit.SECONDS);
            assertNotNull(iotReply);
            assertEquals("IReply", iotReply.getHeader().getName());
            assertEquals(iotCid, iotReply.getCorrelationId());

            provider.unsubscribe(localReq);
            provider.unsubscribeFromIoTCore(iotReq);
        } finally {
            provider.close();
        }
    }
}
