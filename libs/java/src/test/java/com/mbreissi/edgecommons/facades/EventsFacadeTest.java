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

import java.time.Clock;
import java.time.Instant;
import java.time.ZoneOffset;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Deterministic unit tests for {@link EventsFacade} — the {@code events()} facade
 * (DESIGN-class-facades §2.2, D8): the {@code evt/{severity}/{type}} channel DERIVED from the body
 * (topic + body can never disagree), the {@code timestamp} → now default, {@code emit} +
 * {@code raiseAlarm}/{@code clearAlarm}, and the local/northbound routing (stream rejected).
 */
class EventsFacadeTest {

    private static final String NOW = "2026-07-01T12:00:00Z";
    private static final Clock CLOCK = Clock.fixed(Instant.parse(NOW), ZoneOffset.UTC);
    private static final MessageIdentity IDENTITY = new MessageIdentity(
            List.of(new MessageIdentity.HierEntry("device", "gw-01")), "opcua-adapter", "main");

    private MockMessagingService messaging;
    private EventsFacade facade;

    @BeforeEach
    void setUp() {
        messaging = new MockMessagingService();
        MockConfigurationService config = new MockConfigurationService();
        config.setComponentIdentity(IDENTITY);
        Uns uns = new Uns(IDENTITY, false);
        facade = new EventsFacade(config, "main", uns, messaging, CLOCK);
    }

    private MockMessagingService.PublishedMessage last() {
        List<MockMessagingService.PublishedMessage> published = messaging.getPublishedMessages();
        return published.get(published.size() - 1);
    }

    private JsonObject lastBody() {
        return last().message.toDict().getAsJsonObject("body");
    }

    @Test
    void emitDerivesChannelFromSeverityAndTypeAndDefaultsTimestamp() {
        facade.emit(Severity.WARNING, "write-rejected", "not in allow-list", null);

        assertEquals("ecv1/gw-01/opcua-adapter/main/evt/warning/write-rejected", last().topic);
        JsonObject body = lastBody();
        assertEquals("warning", body.get("severity").getAsString());
        assertEquals("write-rejected", body.get("type").getAsString());
        assertEquals("not in allow-list", body.get("message").getAsString());
        assertEquals(NOW, body.get("timestamp").getAsString());
        assertFalse(body.has("context"), "an omitted context is absent, not an empty object");
        assertFalse(body.has("alarm"), "a plain event carries no alarm/active");
    }

    @Test
    void messageOnlyEmitDefaultsSeverityToInfo() {
        facade.emit("door-open", "front door opened");

        assertEquals("ecv1/gw-01/opcua-adapter/main/evt/info/door-open", last().topic);
        assertEquals("info", lastBody().get("severity").getAsString());
    }

    @Test
    void contextIsIncludedWhenProvided() {
        JsonObject ctx = JsonParser.parseString("{\"celsius\":95.0}").getAsJsonObject();
        facade.emit(Severity.CRITICAL, "overtemp", "too hot", ctx);

        assertEquals(ctx, lastBody().getAsJsonObject("context"));
    }

    @Test
    void typeIsSanitizedForTheChannelButRidesTheBodyVerbatim() {
        facade.emit(Severity.INFO, "a+b", null, null);

        assertEquals("ecv1/gw-01/opcua-adapter/main/evt/info/a_b", last().topic);
        assertEquals("a+b", lastBody().get("type").getAsString());
    }

    @Test
    void raiseAlarmDefaultsToCriticalWithAlarmActiveTrue() {
        facade.raiseAlarm("connection-lost", "link down",
                JsonParser.parseString("{\"connected\":false}").getAsJsonObject());

        assertEquals("ecv1/gw-01/opcua-adapter/main/evt/critical/connection-lost", last().topic);
        JsonObject body = lastBody();
        assertEquals("critical", body.get("severity").getAsString());
        assertTrue(body.get("alarm").getAsBoolean());
        assertTrue(body.get("active").getAsBoolean());
    }

    @Test
    void clearAlarmSharesTheRaiseChannelWithActiveFalse() {
        facade.clearAlarm("connection-lost", null);

        assertEquals("ecv1/gw-01/opcua-adapter/main/evt/critical/connection-lost", last().topic,
                "raise and clear ride the same evt/critical/{type} channel");
        JsonObject body = lastBody();
        assertTrue(body.get("alarm").getAsBoolean());
        assertFalse(body.get("active").getAsBoolean());
        assertFalse(body.has("message"), "clearAlarm carries no message");
    }

    @Test
    void alarmSeverityIsOverridable() {
        facade.raiseAlarm(Severity.WARNING, "degraded", "running degraded", null);
        assertEquals("ecv1/gw-01/opcua-adapter/main/evt/warning/degraded", last().topic);
    }

    @Test
    void viaNorthboundRoutesToIoTCore() {
        facade.via(Channel.NORTHBOUND).emit(Severity.CRITICAL, "overtemp", "escalate", null);
        assertEquals(Qos.AT_LEAST_ONCE, last().qos);
    }

    @Test
    void viaStreamIsRejected() {
        assertThrows(IllegalArgumentException.class, () -> facade.via(Channel.stream("hot")));
    }

    @Test
    void emptyTypeIsRejected() {
        assertThrows(IllegalArgumentException.class,
                () -> facade.emit(Severity.INFO, "", "msg", null));
        assertTrue(messaging.getPublishedMessages().isEmpty());
    }

    @Test
    void buildBodyCoversTheFourSeverityWireTokens() {
        assertEquals("critical", Severity.CRITICAL.wire());
        assertEquals("warning", Severity.WARNING.wire());
        assertEquals("info", Severity.INFO.wire());
        assertEquals("debug", Severity.DEBUG.wire());
        assertEquals(Severity.DEBUG, Severity.fromWire("debug"));
        assertNull(Severity.fromWire("nope"));
    }
}
