/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons;

import com.mbreissi.edgecommons.facades.AppFacade;
import com.mbreissi.edgecommons.facades.DataFacade;
import com.mbreissi.edgecommons.facades.EventsFacade;
import com.mbreissi.edgecommons.facades.Severity;
import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.mbreissi.edgecommons.test.MockConfigurationService;
import com.mbreissi.edgecommons.test.MockMessagingService;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.lang.reflect.Field;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNotSame;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;

/**
 * Wiring tests for the publish-facade accessors (DESIGN-class-facades §3, D6): the instance-bound
 * {@code gg.instance(id).data()/events()/app()} (primary) and the component-scope convenience
 * {@code gg.getData()/getEvents()/getApp()} (D‑U28: no instance token — topics carry no instance
 * slot). Fields are injected via the protected no-arg constructor so nothing connects to IPC/MQTT
 * (the pattern {@code EdgeCommonsFacadeUnitTest} uses).
 */
class FacadeAccessorTest {

    private static final MessageIdentity IDENTITY = new MessageIdentity(
            List.of(new MessageIdentity.HierEntry("device", "gw-01")), "opcua-adapter", "main");

    private EdgeCommons gg;
    private MockMessagingService messaging;

    private static void setField(EdgeCommons gg, String name, Object value) throws Exception {
        Field f = EdgeCommons.class.getDeclaredField(name);
        f.setAccessible(true);
        f.set(gg, value);
    }

    @BeforeEach
    void setUp() throws Exception {
        var ctor = EdgeCommons.class.getDeclaredConstructor();
        ctor.setAccessible(true);
        gg = ctor.newInstance();
        MockConfigurationService config = new MockConfigurationService();
        config.setComponentIdentity(IDENTITY);
        messaging = new MockMessagingService();
        setField(gg, "configManager", config);
        setField(gg, "messagingClient", messaging);
    }

    @Test
    void convenienceAccessorsAreCachedComponentScopeFacades() {
        DataFacade data = gg.getData();
        EventsFacade events = gg.getEvents();
        AppFacade app = gg.getApp();
        assertNotNull(data);
        assertNotNull(events);
        assertNotNull(app);

        // The convenience accessors are cached (same object each call)...
        assertSame(data, gg.getData());
        assertSame(events, gg.getEvents());
        assertSame(app, gg.getApp());
        // ...and are the component-scope facades, distinct from an instance-bound facade (D‑U28).
        assertNotSame(data, gg.instance("main").data());
        assertNotSame(events, gg.instance("main").events());
        assertNotSame(app, gg.instance("main").app());
    }

    @Test
    void componentBoundDataPublishesOnTheComponentScopeTopic() {
        gg.getData().publish("temp", 21.5);

        MockMessagingService.PublishedMessage pm = messaging.getPublishedMessages().get(0);
        // D‑U28: component scope — no instance slot in the topic and no instance key in the identity.
        assertEquals("ecv1/gw-01/opcua-adapter/data/temp", pm.topic);
        assertNull(pm.message.getIdentity().getInstance());
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

        assertEquals("ecv1/gw-01/opcua-adapter/evt/info/started",
                messaging.getPublishedMessages().get(0).topic);
        assertEquals("ecv1/gw-01/opcua-adapter/app/hi",
                messaging.getPublishedMessages().get(1).topic);
    }
}
