/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import com.google.gson.JsonObject;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import com.mbreissi.edgecommons.messaging.Qos;

import java.lang.reflect.Field;
import java.time.Duration;
import java.util.function.BiConsumer;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

/**
 * The reserved-class publish guard (UNS-CANONICAL-DESIGN §4.1, D-U4/D-U8/D-U24): every public
 * client-chosen-topic publish path on {@link MessagingClient} rejects topics targeting a reserved
 * UNS class ({@code state | metric | cfg | log}); position 4 is checked always, position 5 only
 * when {@code topic.includeRoot} is late-bound true; non-{@code ecv1} topics (reply topics, the
 * cloudwatch component contract), {@code app} channels that merely END in a reserved token, and
 * all {@code subscribe*} calls pass; and the privileged {@link ReservedPublisher} seam bypasses
 * the guard entirely (§4.2).
 */
class ReservedTopicGuardTest {

    private static final String RESERVED_STATE = "ecv1/gw-01/opcua-adapter/main/state";
    private static final String RESERVED_METRIC = "ecv1/gw-01/opcua-adapter/main/metric/m1";
    private static final String RESERVED_CFG = "ecv1/gw-01/opcua-adapter/main/cfg";
    private static final String RESERVED_LOG = "ecv1/gw-01/opcua-adapter/main/log/tail";
    private static final String ALLOWED_DATA = "ecv1/gw-01/opcua-adapter/main/data/temp";

    private MessagingProvider provider;
    private MessagingClient client;

    @BeforeEach
    void setUp() throws Exception {
        provider = mock(MessagingProvider.class);
        client = new MessagingClient() { };
        Field f = MessagingClient.class.getDeclaredField("messagingProvider");
        f.setAccessible(true);
        f.set(client, provider);
    }

    private static Message message() {
        JsonObject body = new JsonObject();
        body.addProperty("k", "v");
        return MessageBuilder.fromObject(body);
    }

    /** A request message whose reply_to is the given topic (the hostile-reply_to case, D-U8). */
    private static Message requestWithReplyTo(String replyTopic) {
        Message request = message();
        request.makeRequest(replyTopic);
        return request;
    }

    // ----- every guarded method rejects a reserved topic -----

    @Test
    void publishRejectsReservedTopics() {
        for (String topic : new String[]{RESERVED_STATE, RESERVED_METRIC, RESERVED_CFG, RESERVED_LOG}) {
            ReservedTopicException ex = assertThrows(ReservedTopicException.class,
                    () -> client.publish(topic, message()), topic);
            assertEquals(topic, ex.getTopic());
        }
        verifyNoInteractions(provider);
    }

    @Test
    void publishRawRejectsReservedTopics() {
        assertThrows(ReservedTopicException.class,
                () -> client.publishRaw(RESERVED_METRIC, new JsonObject()));
        verifyNoInteractions(provider);
    }

    @Test
    void publishNorthboundRejectsReservedTopics() {
        assertThrows(ReservedTopicException.class,
                () -> client.publishNorthbound(RESERVED_STATE, message(), Qos.AT_LEAST_ONCE));
        verifyNoInteractions(provider);
    }

    @Test
    void publishNorthboundRawRejectsReservedTopics() {
        assertThrows(ReservedTopicException.class,
                () -> client.publishNorthboundRaw(RESERVED_CFG, new JsonObject(), Qos.AT_MOST_ONCE));
        verifyNoInteractions(provider);
    }

    @Test
    void requestRejectsReservedTopicsBothOverloads() {
        assertThrows(ReservedTopicException.class, () -> client.request(RESERVED_STATE, message()));
        assertThrows(ReservedTopicException.class,
                () -> client.request(RESERVED_STATE, message(), Duration.ofSeconds(1)));
        verifyNoInteractions(provider);
    }

    @Test
    void requestNorthboundRejectsReservedTopicsBothOverloads() {
        assertThrows(ReservedTopicException.class,
                () -> client.requestNorthbound(RESERVED_LOG, message()));
        assertThrows(ReservedTopicException.class,
                () -> client.requestNorthbound(RESERVED_LOG, message(), Duration.ZERO));
        verifyNoInteractions(provider);
    }

    @Test
    void replyRejectsAHostileReservedReplyTo() {
        // A hostile requester setting header.reply_to to a victim's reserved topic must not turn
        // this responder into a forger (§4.1).
        assertThrows(ReservedTopicException.class,
                () -> client.reply(requestWithReplyTo(RESERVED_STATE), message()));
        assertThrows(ReservedTopicException.class,
                () -> client.replyNorthbound(requestWithReplyTo(RESERVED_METRIC), message()));
        verifyNoInteractions(provider);
    }

    // ----- what passes -----

    @Test
    void nonReservedUnsClassesPass() {
        client.publish(ALLOWED_DATA, message());
        verify(provider).publish(eq(ALLOWED_DATA), any(Message.class));
        client.publish("ecv1/gw-01/opcua-adapter/main/cmd/get-configuration", message());
        client.publish("ecv1/gw-01/opcua-adapter/main/evt/started", message());
        client.publish("ecv1/gw-01/opcua-adapter/main/app/anything", message());
    }

    @Test
    void appChannelEndingInAReservedTokenPasses() {
        // 'state' at position 5 is an app CHANNEL, not the class, when includeRoot is false.
        String topic = "ecv1/gw-01/opcua-adapter/main/app/state";
        client.publish(topic, message());
        verify(provider).publish(eq(topic), any(Message.class));
    }

    @Test
    void nonEcv1TopicsPassUntouched() {
        String[] topics = {
                "edgecommons/reply-abc123",                       // reply topics (D-U6)
                "cloudwatch/metric/put",                        // the external AWS contract (D-U21)
                "edgecommons/thing-1/config/get/MyComponent",     // pre-UNS legacy topic shape (retired in slice 1e) — non-ecv1, passes untouched
                "some/foreign/broker/topic/state",              // foreign bridging
                "ecv1x/a/b/c/state/d",                          // ecv1-prefixed but not the root token
        };
        for (String topic : topics) {
            client.publish(topic, message());
            verify(provider).publish(eq(topic), any(Message.class));
        }
    }

    @Test
    void shortEcv1TopicsPass() {
        // Fewer than 5 levels -> no class position to check.
        for (String topic : new String[]{"ecv1", "ecv1/gw-01", "ecv1/gw-01/comp/main"}) {
            client.publish(topic, message());
            verify(provider).publish(eq(topic), any(Message.class));
        }
    }

    @Test
    void replyWithNonReservedOrAbsentReplyToPasses() {
        Message okRequest = requestWithReplyTo("edgecommons/reply-xyz");
        client.reply(okRequest, message());
        verify(provider).reply(same(okRequest), any(Message.class));
    }

    @Test
    void subscribeIsNeverGuarded() {
        // Consumers must be able to read reserved classes.
        BiConsumer<String, Message> cb = (t, m) -> { };
        client.subscribe("ecv1/+/+/+/state", cb);
        verify(provider).subscribe(eq("ecv1/+/+/+/state"), eq(cb), anyInt(), anyInt());
        client.subscribeNorthbound(RESERVED_STATE, cb, Qos.AT_LEAST_ONCE);
        verify(provider).subscribeNorthbound(eq(RESERVED_STATE), eq(cb), eq(Qos.AT_LEAST_ONCE),
                anyInt(), anyInt());
    }

    // ----- position 4 vs position 5 and the includeRoot late-bind (D-U24) -----

    @Test
    void position5IsOnlyCheckedWhenIncludeRootIsBoundTrue() {
        String rootedReserved = "ecv1/dallas/gw-01/opcua-adapter/main/state";

        // Default (pre-late-bind): includeRoot=false -> position 5 not checked -> passes.
        client.publish(rootedReserved, message());
        verify(provider).publish(eq(rootedReserved), any(Message.class));

        // After the late-bind (EdgeCommons.init right after ConfigManager): rejected.
        client.setGuardIncludeRoot(true);
        ReservedTopicException ex = assertThrows(ReservedTopicException.class,
                () -> client.publish(rootedReserved, message()));
        assertEquals("state", ex.getClassToken());

        // Position 4 stays guarded regardless of includeRoot.
        assertThrows(ReservedTopicException.class, () -> client.publish(RESERVED_STATE, message()));

        // And an includeRoot component's own app/... channel at position 5 is legitimately
        // rejected only when the token at position 5 is reserved - 'app' there passes.
        client.publish("ecv1/dallas/gw-01/opcua-adapter/main/app", message());

        // Unbinding restores position-4-only checking.
        client.setGuardIncludeRoot(false);
        client.publish(rootedReserved, message());
    }

    @Test
    void reservedClassOfPredicateMatchesTheSpec() {
        // reject if tokens[0]=="ecv1" && (len>=5 && reserved(tokens[4]) || includeRoot && len>=6 && reserved(tokens[5]))
        assertNotNull(MessagingClient.reservedClassOf("ecv1/d/c/i/state", false));
        assertNotNull(MessagingClient.reservedClassOf("ecv1/d/c/i/metric/m", false));
        assertNotNull(MessagingClient.reservedClassOf("ecv1/d/c/i/cfg", false));
        assertNotNull(MessagingClient.reservedClassOf("ecv1/d/c/i/log/tail", false));
        assertNull(MessagingClient.reservedClassOf("ecv1/d/c/i/data/x", false));
        assertNull(MessagingClient.reservedClassOf("ecv1/d/c/i/app/state", false));
        assertNotNull(MessagingClient.reservedClassOf("ecv1/s/d/c/i/state", true));
        assertNull(MessagingClient.reservedClassOf("ecv1/s/d/c/i/state", false));
        assertNull(MessagingClient.reservedClassOf("other/d/c/i/state", true));
        assertNull(MessagingClient.reservedClassOf(null, true));
        assertNull(MessagingClient.reservedClassOf("ecv1/d/c/i", true));
    }

    // ----- the privileged seam bypasses (§4.2) -----

    @Test
    void reservedPublisherBypassesTheGuard() {
        ReservedPublisher publisher = client.reservedPublisher();
        assertSame(publisher, client.reservedPublisher(), "the seam is cached per client");

        publisher.publish(RESERVED_STATE, message());
        verify(provider).publish(eq(RESERVED_STATE), any(Message.class));

        JsonObject raw = new JsonObject();
        publisher.publishRaw(RESERVED_METRIC, raw);
        verify(provider).publishRaw(RESERVED_METRIC, raw);

        publisher.publishNorthbound(RESERVED_CFG, message(), Qos.AT_LEAST_ONCE);
        verify(provider).publishNorthbound(eq(RESERVED_CFG), any(Message.class), eq(Qos.AT_LEAST_ONCE));
    }

    @Test
    void reservedPublisherBypassesEvenWithIncludeRootBound() {
        client.setGuardIncludeRoot(true);
        String rootedReserved = "ecv1/dallas/gw-01/opcua-adapter/main/state";
        client.reservedPublisher().publish(rootedReserved, message());
        verify(provider).publish(eq(rootedReserved), any(Message.class));
    }
}
