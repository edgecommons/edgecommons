/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import com.mbreissi.edgecommons.config.ConfigManager;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.when;

/**
 * Envelope wiring tests for the UNS {@code identity} element: toDict()/fromObject round-trips
 * (with the canonical header, identity, tags, body order), the extended envelope-detection
 * predicate, lenient inbound parsing, and the {@link MessageBuilder} stamping rules
 * (explicit override &gt; config-resolved identity + instance token &gt; none).
 */
class MessageIdentityEnvelopeTest {

    private static MessageIdentity testIdentity() {
        return new MessageIdentity(
                List.of(
                        new MessageIdentity.HierEntry("site", "dallas"),
                        new MessageIdentity.HierEntry("device", "gw-01")),
                "test-component", "main");
    }

    private static ConfigManager configWithIdentity(MessageIdentity identity) {
        ConfigManager cm = mock(ConfigManager.class);
        when(cm.getTagConfig()).thenReturn(null);
        when(cm.getComponentIdentity()).thenReturn(identity);
        return cm;
    }

    // ----- envelope round-trip -----

    @Test
    void toDictEmitsIdentityBetweenHeaderAndTags() {
        Message message = MessageBuilder.create("Evt", "1.0")
                .withPayload(new JsonObject())
                .withConfig(configWithIdentity(testIdentity()))
                .build();

        JsonObject dict = message.toDict();
        assertEquals(List.of("header", "identity", "tags", "body"), List.copyOf(dict.keySet()),
                "canonical envelope member order: header, identity, tags, body");
        assertEquals("gw-01", dict.getAsJsonObject("identity")
                .getAsJsonArray("hier").get(1).getAsJsonObject().get("value").getAsString());
    }

    @Test
    void envelopeRoundTripPreservesIdentity() {
        Message original = MessageBuilder.create("Evt", "1.0")
                .withPayload(new JsonObject())
                .withConfig(configWithIdentity(testIdentity()))
                .withInstance("kep1")
                .build();

        Message parsed = MessageBuilder.fromObject(original.toDict());

        assertNotNull(parsed.getIdentity());
        assertEquals("dallas/gw-01", parsed.getIdentity().getPath());
        assertEquals("test-component", parsed.getIdentity().getComponent());
        assertEquals("kep1", parsed.getIdentity().getInstance());
        assertEquals("gw-01", parsed.getIdentity().getDevice());
        assertEquals(original.getIdentity().getHier(), parsed.getIdentity().getHier());
        assertNull(parsed.getRaw());
    }

    @Test
    void identityOnlyObjectIsDetectedAsEnvelopeNotRaw() {
        // The envelope-detection predicate is header | identity | tags | body.
        JsonObject msg = new JsonObject();
        msg.add("identity", testIdentity().toDict());

        Message parsed = MessageBuilder.fromObject(msg);

        assertNull(parsed.getRaw(), "an identity-carrying object is an envelope, not raw");
        assertNotNull(parsed.getIdentity());
        assertEquals("test-component", parsed.getIdentity().getComponent());
    }

    @Test
    void deprecatedMessageBuildAlsoParsesIdentity() {
        JsonObject msg = new JsonObject();
        JsonObject header = new JsonObject();
        header.addProperty("name", "Evt");
        header.addProperty("version", "1.0");
        msg.add("header", header);
        msg.add("identity", testIdentity().toDict());

        @SuppressWarnings("deprecation")
        Message parsed = Message.build(msg);

        assertNotNull(parsed.getIdentity());
        assertEquals("dallas/gw-01", parsed.getIdentity().getPath());
        assertNull(parsed.getRaw());
    }

    @Test
    void malformedInboundIdentityIsDroppedButMessageDelivers() {
        JsonObject msg = new JsonObject();
        JsonObject header = new JsonObject();
        header.addProperty("name", "Evt");
        header.addProperty("version", "1.0");
        msg.add("header", header);
        // identity present but not an object -> lenient: identity null, message still parses
        msg.addProperty("identity", "not-an-object");
        msg.add("body", new JsonObject());

        Message parsed = MessageBuilder.fromObject(msg);

        assertNull(parsed.getIdentity());
        assertNotNull(parsed.getHeader());
        assertEquals("Evt", parsed.getHeader().getName());
        assertNull(parsed.getRaw());

        // malformed hier inside an identity object -> same leniency
        JsonObject msg2 = new JsonObject();
        msg2.add("header", header);
        JsonObject badIdentity = new JsonObject();
        badIdentity.add("hier", new JsonArray());
        msg2.add("identity", badIdentity);
        Message parsed2 = MessageBuilder.fromObject(msg2);
        assertNull(parsed2.getIdentity());
        assertNotNull(parsed2.getHeader());
    }

    @Test
    void rawMessageNeverCarriesIdentity() {
        Message raw = MessageBuilder.fromObject("plain string payload");
        assertNull(raw.getIdentity());
        JsonObject dict = raw.toDict();
        assertTrue(dict.has("raw"));
        assertEquals(1, dict.size());
    }

    // ----- builder stamping -----

    @Test
    void buildWithConfigStampsComponentScopeIdentityByDefault() {
        Message message = MessageBuilder.create("Evt", "1.0")
                .withPayload("x")
                .withConfig(configWithIdentity(testIdentity()))
                .build();

        assertNotNull(message.getIdentity());
        // D‑U28: with no explicit withInstance token, the message is component-scoped (no instance).
        assertNull(message.getIdentity().getInstance());
        assertEquals("test-component", message.getIdentity().getComponent());
        assertEquals("gw-01", message.getIdentity().getDevice());
    }

    @Test
    void buildWithInstanceStampsInstanceToken() {
        Message message = MessageBuilder.create("Evt", "1.0")
                .withPayload("x")
                .withConfig(configWithIdentity(testIdentity()))
                .withInstance("kep1")
                .build();

        assertNotNull(message.getIdentity());
        assertEquals("kep1", message.getIdentity().getInstance());
    }

    @Test
    void buildWithIdentityOverrideWinsOverConfig() {
        MessageIdentity override = new MessageIdentity(
                List.of(new MessageIdentity.HierEntry("device", "other-device")),
                "relay-component", "relay");

        Message message = MessageBuilder.create("Evt", "1.0")
                .withPayload("x")
                .withConfig(configWithIdentity(testIdentity()))
                .withInstance("ignored-for-override")
                .withIdentity(override)
                .build();

        assertSame(override, message.getIdentity());
        assertEquals("relay", message.getIdentity().getInstance());
    }

    @Test
    void buildWithoutConfigOrOverrideLeavesIdentityNull() {
        Message message = MessageBuilder.create("Evt", "1.0")
                .withPayload("x")
                .build();

        assertNull(message.getIdentity());
        assertNull(message.getTags());
        assertNotNull(message.getHeader());
    }

    @Test
    void buildWithConfigWithoutResolvedIdentityLeavesIdentityNull() {
        // A config service whose identity is unresolved (test/subclass bring-up) must not NPE.
        ConfigManager cm = mock(ConfigManager.class);
        when(cm.getTagConfig()).thenReturn(null);
        when(cm.getComponentIdentity()).thenReturn(null);

        Message message = MessageBuilder.create("Evt", "1.0")
                .withPayload("x")
                .withConfig(cm)
                .build();

        assertNull(message.getIdentity());
        assertNotNull(message.getTags());
    }

    @Test
    void withInstanceNullOrEmptyStampsComponentScope() {
        // D‑U28: a null/empty instance token means component scope (no instance key), so the
        // component-scope publish facades (gg.getData()/getEvents()/getApp()) can build messages.
        Message message = MessageBuilder.create("Evt", "1.0")
                .withPayload("x")
                .withConfig(configWithIdentity(testIdentity()))
                .withInstance(null)
                .build();
        assertNotNull(message.getIdentity());
        assertNull(message.getIdentity().getInstance());

        Message empty = MessageBuilder.create("Evt", "1.0")
                .withPayload("x")
                .withConfig(configWithIdentity(testIdentity()))
                .withInstance("")
                .build();
        assertNull(empty.getIdentity().getInstance());
    }
}
