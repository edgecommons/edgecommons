/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.facades;

import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.mbreissi.edgecommons.test.MockConfigurationService;
import com.mbreissi.edgecommons.test.MockMessagingService;
import com.mbreissi.edgecommons.uns.Uns;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import com.mbreissi.edgecommons.messaging.Qos;

import java.util.List;

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
}
