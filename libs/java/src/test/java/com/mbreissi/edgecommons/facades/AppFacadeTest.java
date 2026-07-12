/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.facades;

import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.test.MockConfigurationService;
import com.mbreissi.edgecommons.test.MockMessagingService;
import com.mbreissi.edgecommons.uns.Uns;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import com.mbreissi.edgecommons.messaging.Qos;

import java.util.List;
import java.time.Duration;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Deterministic unit tests for {@link AppFacade} — the {@code app()} facade (DESIGN-class-facades
 * §2.3, D3): the thin-sugar guarantee (named header + verbatim body onto {@code app/{channel}},
 * identity stamped), channel sanitization, and the local/northbound routing (stream rejected).
 */
class AppFacadeTest {

    private static final MessageIdentity IDENTITY = new MessageIdentity(
            List.of(new MessageIdentity.HierEntry("device", "gw-01")), "opcua-adapter", "main");

    private MockMessagingService messaging;
    private AppFacade facade;

    @BeforeEach
    void setUp() {
        messaging = new MockMessagingService();
        MockConfigurationService config = new MockConfigurationService();
        config.setComponentIdentity(IDENTITY);
        Uns uns = new Uns(IDENTITY, false);
        facade = new AppFacade(config, "main", uns, messaging);
    }

    private MockMessagingService.PublishedMessage last() {
        List<MockMessagingService.PublishedMessage> published = messaging.getPublishedMessages();
        return published.get(published.size() - 1);
    }

    @Test
    void publishesVerbatimBodyWithNamedHeaderOntoAppChannel() {
        JsonObject body = JsonParser.parseString("{\"orderId\":\"A-42\",\"qty\":3}").getAsJsonObject();
        facade.publish("OrderReceived", "order/received", body);

        MockMessagingService.PublishedMessage pm = last();
        assertEquals("ecv1/gw-01/opcua-adapter/main/app/order/received", pm.topic);
        assertEquals("OrderReceived", pm.message.getHeader().getName(),
                "the header name is the caller's chosen name");
        assertEquals(body, pm.message.toDict().getAsJsonObject("body"), "body rides verbatim");
    }

    @Test
    void channelIsSanitized() {
        facade.publish("Ping", "a+b", new JsonObject());
        assertEquals("ecv1/gw-01/opcua-adapter/main/app/a_b", last().topic);
    }

    @Test
    void northboundRoutingGoesToIoTCore() {
        facade.publish("CloudEvent", "cloud", new JsonObject(), Channel.NORTHBOUND);
        assertEquals(Qos.AT_LEAST_ONCE, last().qos);
    }

    @Test
    void streamRoutingIsRejected() {
        assertThrows(IllegalArgumentException.class,
                () -> facade.publish("X", "c", new JsonObject(), Channel.stream("hot")));
    }

    @Test
    void emptyNameOrChannelIsRejected() {
        assertThrows(IllegalArgumentException.class,
                () -> facade.publish("", "c", new JsonObject()));
        assertThrows(IllegalArgumentException.class,
                () -> facade.publish("X", "", new JsonObject()));
        assertTrue(messaging.getPublishedMessages().isEmpty());
    }

    @Test
    void prepareCapturesTopicEnvelopeAndExactDefensiveBytes() {
        JsonObject body = JsonParser.parseString("{\"captureId\":\"cap-1\"}")
                .getAsJsonObject();
        AppFacade.PreparedAppMessage prepared =
                facade.prepare("ImageCaptured", "image/captured", body);

        assertEquals("ecv1/gw-01/opcua-adapter/main/app/image/captured", prepared.topic());
        Message decoded = Message.fromBytes(prepared.encodedBytes());
        assertEquals(prepared.message().getHeader().getUuid(), decoded.getHeader().getUuid());
        assertEquals(body, decoded.toDict().getAsJsonObject("body"));

        byte[] original = prepared.encodedBytes();
        byte[] first = prepared.encodedBytes();
        first[0] ^= 0x7f;
        assertArrayEquals(original, prepared.encodedBytes(),
                "caller mutation must not alter retained bytes");
    }

    @Test
    void prepareCorrelatedAcceptsRequestOrExplicitCorrelation() {
        Message request = MessageBuilder.create("sb/capture", "1.0")
                .withCorrelationId("corr-request")
                .withPayload(new JsonObject())
                .build();
        AppFacade.PreparedAppMessage fromRequest = facade.prepareCorrelated(
                "ImageCaptured", "image/captured", new JsonObject(), request);
        AppFacade.PreparedAppMessage explicit = facade.prepareCorrelated(
                "ImageCaptured", "image/captured", new JsonObject(), "corr-explicit");

        assertEquals("corr-request",
                fromRequest.message().getHeader().getCorrelationId());
        assertEquals("corr-explicit", explicit.message().getHeader().getCorrelationId());
        assertThrows(IllegalArgumentException.class, () -> facade.prepareCorrelated(
                "X", "c", new JsonObject(), (String) null));
        assertThrows(IllegalArgumentException.class, () -> facade.prepareCorrelated(
                "X", "c", new JsonObject(), MessageBuilder.fromObject(new JsonObject())));
    }

    @Test
    void confirmedPreparedPublishUsesExactBytesQosOneAndRouting() {
        AppFacade.PreparedAppMessage prepared = facade.prepareCorrelated(
                "ImageCaptured", "image/captured", new JsonObject(), "corr-1");
        byte[] exact = prepared.encodedBytes();
        // Mutating the diagnostic Message view after preparation must not reconstruct the outbox
        // payload or change the UUID/correlation already captured in exact bytes.
        prepared.message().setCorrelationId("mutated-after-prepare");

        facade.publishConfirmed(prepared, Duration.ofSeconds(2));
        MockMessagingService.ConfirmedPublish local = messaging.getConfirmedPublishes().get(0);
        assertArrayEquals(exact, local.encodedBytes);
        assertEquals(Qos.AT_LEAST_ONCE, local.qos);
        assertEquals(Duration.ofSeconds(2), local.timeout);
        assertTrue(!local.northbound);

        messaging.clearPublishedMessages();
        facade.publishConfirmed(prepared, Channel.NORTHBOUND, Duration.ofSeconds(3));
        MockMessagingService.ConfirmedPublish northbound =
                messaging.getConfirmedPublishes().get(0);
        assertArrayEquals(exact, northbound.encodedBytes);
        assertTrue(northbound.northbound);
        assertEquals(Qos.AT_LEAST_ONCE, northbound.qos);
    }
}
