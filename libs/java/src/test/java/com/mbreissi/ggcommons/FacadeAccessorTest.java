/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons;

import com.mbreissi.ggcommons.facades.AppFacade;
import com.mbreissi.ggcommons.facades.DataFacade;
import com.mbreissi.ggcommons.facades.EventsFacade;
import com.mbreissi.ggcommons.facades.Severity;
import com.mbreissi.ggcommons.messaging.MessageIdentity;
import com.mbreissi.ggcommons.test.MockConfigurationService;
import com.mbreissi.ggcommons.test.MockMessagingService;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.lang.reflect.Field;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertSame;

/**
 * Wiring tests for the publish-facade accessors (DESIGN-class-facades §3, D6): the instance-bound
 * {@code gg.instance(id).data()/events()/app()} (primary) and the {@code main}-instance convenience
 * {@code gg.getData()/getEvents()/getApp()} (== {@code instance("main")}). Fields are injected via
 * the protected no-arg constructor so nothing connects to IPC/MQTT (the pattern
 * {@code GGCommonsFacadeUnitTest} uses).
 */
class FacadeAccessorTest {

    private static final MessageIdentity IDENTITY = new MessageIdentity(
            List.of(new MessageIdentity.HierEntry("device", "gw-01")), "opcua-adapter", "main");

    private GGCommons gg;
    private MockMessagingService messaging;

    private static void setField(GGCommons gg, String name, Object value) throws Exception {
        Field f = GGCommons.class.getDeclaredField(name);
        f.setAccessible(true);
        f.set(gg, value);
    }

    @BeforeEach
    void setUp() throws Exception {
        var ctor = GGCommons.class.getDeclaredConstructor();
        ctor.setAccessible(true);
        gg = ctor.newInstance();
        MockConfigurationService config = new MockConfigurationService();
        config.setComponentIdentity(IDENTITY);
        messaging = new MockMessagingService();
        setField(gg, "configManager", config);
        setField(gg, "messagingClient", messaging);
    }

    @Test
    void convenienceAccessorsEqualTheMainInstanceFacades() {
        DataFacade data = gg.getData();
        EventsFacade events = gg.getEvents();
        AppFacade app = gg.getApp();
        assertNotNull(data);
        assertNotNull(events);
        assertNotNull(app);

        // gg.getData() == gg.instance("main").data(), and cached (same object each call).
        assertSame(data, gg.instance("main").data());
        assertSame(events, gg.instance("main").events());
        assertSame(app, gg.instance("main").app());
        assertSame(data, gg.getData());
    }

    @Test
    void componentBoundDataPublishesOnTheMainInstanceTopic() {
        gg.getData().publish("temp", 21.5);

        MockMessagingService.PublishedMessage pm = messaging.getPublishedMessages().get(0);
        assertEquals("ecv1/gw-01/opcua-adapter/main/data/temp", pm.topic);
        assertEquals("main", pm.message.getIdentity().getInstance());
    }

    @Test
    void instanceBoundDataStampsTheInstanceToken() {
        gg.instance("kep1").data().publish("temp", 21.5);

        MockMessagingService.PublishedMessage pm = messaging.getPublishedMessages().get(0);
        assertEquals("ecv1/gw-01/opcua-adapter/kep1/data/temp", pm.topic);
        assertEquals("kep1", pm.message.getIdentity().getInstance());
    }

    @Test
    void componentBoundEventsAndAppPublishOnTheirClasses() {
        gg.getEvents().emit(Severity.INFO, "started", "up", null);
        gg.getApp().publish("Hello", "hi", new com.google.gson.JsonObject());

        assertEquals("ecv1/gw-01/opcua-adapter/main/evt/info/started",
                messaging.getPublishedMessages().get(0).topic);
        assertEquals("ecv1/gw-01/opcua-adapter/main/app/hi",
                messaging.getPublishedMessages().get(1).topic);
    }
}
