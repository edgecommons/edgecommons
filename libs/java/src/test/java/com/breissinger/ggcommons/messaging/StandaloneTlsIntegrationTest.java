/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.messaging;

import com.breissinger.ggcommons.messaging.providers.standalone.StandaloneMessagingProvider;
import com.breissinger.ggcommons.test.MockConfigurationService;
import com.google.gson.Gson;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import java.io.File;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assumptions.assumeTrue;

/**
 * Secure-connection integration test for {@link StandaloneMessagingProvider} against the
 * shared local TLS broker (see the {@code ggcommons-test-infra} repo: an EMQX TLS listener
 * on :8883 using the generated test certs).
 *
 * <p>Self-skips (JUnit assumptions) unless {@code GGCOMMONS_TLS_CERTS_DIR} points at a certs
 * directory containing {@code ca.crt}/{@code client.crt}/{@code client.key} AND the broker is
 * reachable — so a normal {@code mvn verify} (without the broker) stays green. Run it with:
 *
 * <pre>
 * GGCOMMONS_TLS_CERTS_DIR=&lt;infra&gt;/tls-certs \
 *   mvn -Dtest=StandaloneTlsIntegrationTest -DfailIfNoTests=false test
 * </pre>
 */
class StandaloneTlsIntegrationTest {

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

    private static String json(String dir, boolean includeClientCert) {
        String ca = (dir + "/ca.crt").replace("\\", "/");
        var creds = new StringBuilder("{ \"caPath\": \"" + ca + "\"");
        if (includeClientCert) {
            creds.append(", \"certPath\": \"").append((dir + "/client.crt").replace("\\", "/")).append("\"");
            creds.append(", \"keyPath\": \"").append((dir + "/client.key").replace("\\", "/")).append("\"");
        }
        creds.append(" }");
        return """
                { "messaging": { "local": {"host": "localhost", "port": 8883, "clientId": "ggcommons-java-tls-it","credentials": %s } } }"""
                .formatted(creds);
    }

    private static StandaloneMessagingProvider connectOrSkip(boolean mutual) {
        String dir = certsDir();
        assumeTrue(haveCerts(dir), "set GGCOMMONS_TLS_CERTS_DIR to a dir with ca.crt/client.crt/client.key");
        MessagingConfiguration config =
                new Gson().fromJson(json(dir, mutual), MessagingConfiguration.class);
        try {
            return new StandaloneMessagingProvider(config, "ggcommons-java-tls-it");
        } catch (Exception e) {
            assumeTrue(false, "TLS broker not reachable on localhost:8883 (" + e.getMessage() + ")");
            return null; // unreachable
        }
    }

    private Message msg(String name, String key, String value) {
        var payload = new JsonObject();
        payload.addProperty(key, value);
        return MessageBuilder.create(name, "1.0").withPayload(payload).withConfig(CFG).build();
    }

    @Test
    void mutualTlsPublishSubscribeRoundtrip() throws Exception {
        StandaloneMessagingProvider provider = connectOrSkip(true);
        try {
            String topic = "ggcommons/test/tls/java/" + System.nanoTime();
            var latch = new CountDownLatch(1);
            var received = new AtomicReference<Message>();
            provider.subscribe(topic, (t, m) -> { received.set(m); latch.countDown(); }, 1);

            provider.publish(topic, msg("SecureHello", "hello", "secure"));

            assertTrue(latch.await(5, TimeUnit.SECONDS), "message should arrive over TLS");
            assertNotNull(received.get());
            assertEquals("SecureHello", received.get().getHeader().getName());
            provider.unsubscribe(topic);
        } finally {
            provider.close();
        }
    }

    @Test
    void mutualTlsRequestReplyRoundtrip() throws Exception {
        StandaloneMessagingProvider provider = connectOrSkip(true);
        try {
            String reqTopic = "ggcommons/test/tls/java/req/" + System.nanoTime();
            provider.subscribe(reqTopic, (t, request) -> provider.reply(request, msg("Reply", "answer", "42")), 1);

            Message request = msg("Request", "q", "question");
            String correlationId = request.getCorrelationId();
            Message reply = provider.request(reqTopic, request).get(5, TimeUnit.SECONDS);

            assertNotNull(reply);
            assertEquals("Reply", reply.getHeader().getName());
            assertEquals(correlationId, reply.getCorrelationId());
            provider.unsubscribe(reqTopic);
        } finally {
            provider.close();
        }
    }
}
