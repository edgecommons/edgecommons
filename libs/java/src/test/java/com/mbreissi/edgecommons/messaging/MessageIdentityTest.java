/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;

/**
 * Unit tests for {@link MessageIdentity}: construction/validation, path precomputation, the
 * computed device accessor, {@code withInstance} copies, canonical {@code toDict()} order, and
 * the lenient {@code fromDict()} parser (including malformed inputs).
 */
class MessageIdentityTest {

    private static List<MessageIdentity.HierEntry> multiLevelHier() {
        return List.of(
                new MessageIdentity.HierEntry("site", "dallas"),
                new MessageIdentity.HierEntry("zone", "zone-3"),
                new MessageIdentity.HierEntry("device", "gw-01"));
    }

    // ----- construction + accessors -----

    @Test
    void constructorComputesPathAndExposesFields() {
        MessageIdentity id = new MessageIdentity(multiLevelHier(), "opcua-adapter", "kep1");

        assertEquals("dallas/zone-3/gw-01", id.getPath());
        assertEquals("opcua-adapter", id.getComponent());
        assertEquals("kep1", id.getInstance());
        assertEquals(3, id.getHier().size());
        assertEquals("site", id.getHier().get(0).level());
        assertEquals("dallas", id.getHier().get(0).value());
    }

    @Test
    void deviceIsComputedFromLastHierEntry() {
        MessageIdentity id = new MessageIdentity(multiLevelHier(), "comp", null);
        assertEquals("gw-01", id.getDevice());

        MessageIdentity single = new MessageIdentity(
                List.of(new MessageIdentity.HierEntry("device", "thing-1")), "comp", null);
        assertEquals("thing-1", single.getDevice());
        assertEquals("thing-1", single.getPath());
    }

    @Test
    void nullInstanceDefaultsToMain() {
        MessageIdentity id = new MessageIdentity(multiLevelHier(), "comp", null);
        assertEquals(MessageIdentity.DEFAULT_INSTANCE, id.getInstance());
        assertEquals("main", id.getInstance());
    }

    @Test
    void constructorRejectsEmptyHier() {
        assertThrows(IllegalArgumentException.class,
                () -> new MessageIdentity(List.of(), "comp", "main"));
        assertThrows(IllegalArgumentException.class,
                () -> new MessageIdentity(null, "comp", "main"));
    }

    @Test
    void constructorRejectsEmptyComponent() {
        assertThrows(IllegalArgumentException.class,
                () -> new MessageIdentity(multiLevelHier(), null, "main"));
        assertThrows(IllegalArgumentException.class,
                () -> new MessageIdentity(multiLevelHier(), "", "main"));
    }

    @Test
    void hierEntryRejectsEmptyLevelOrValue() {
        assertThrows(IllegalArgumentException.class, () -> new MessageIdentity.HierEntry("", "v"));
        assertThrows(IllegalArgumentException.class, () -> new MessageIdentity.HierEntry(null, "v"));
        assertThrows(IllegalArgumentException.class, () -> new MessageIdentity.HierEntry("l", ""));
        assertThrows(IllegalArgumentException.class, () -> new MessageIdentity.HierEntry("l", null));
    }

    // ----- withInstance -----

    @Test
    void withInstanceReturnsCopyWithNewToken() {
        MessageIdentity id = new MessageIdentity(multiLevelHier(), "comp", "main");
        MessageIdentity copy = id.withInstance("kep1");

        assertEquals("kep1", copy.getInstance());
        assertEquals("main", id.getInstance(), "original must be unchanged (immutability)");
        assertEquals(id.getPath(), copy.getPath());
        assertEquals(id.getComponent(), copy.getComponent());
        assertEquals(id.getHier(), copy.getHier());
    }

    @Test
    void withInstanceRejectsEmptyToken() {
        MessageIdentity id = new MessageIdentity(multiLevelHier(), "comp", "main");
        assertThrows(IllegalArgumentException.class, () -> id.withInstance(null));
        assertThrows(IllegalArgumentException.class, () -> id.withInstance(""));
    }

    // ----- toDict -----

    @Test
    void toDictEmitsCanonicalMemberOrder() {
        MessageIdentity id = new MessageIdentity(multiLevelHier(), "opcua-adapter", "main");
        JsonObject dict = id.toDict();

        assertEquals(List.of("hier", "path", "component", "instance"),
                List.copyOf(dict.keySet()), "canonical member order: hier, path, component, instance");

        JsonArray hier = dict.getAsJsonArray("hier");
        assertEquals(3, hier.size());
        assertEquals("site", hier.get(0).getAsJsonObject().get("level").getAsString());
        assertEquals("dallas", hier.get(0).getAsJsonObject().get("value").getAsString());
        assertEquals("device", hier.get(2).getAsJsonObject().get("level").getAsString());
        assertEquals("gw-01", hier.get(2).getAsJsonObject().get("value").getAsString());

        assertEquals("dallas/zone-3/gw-01", dict.get("path").getAsString());
        assertEquals("opcua-adapter", dict.get("component").getAsString());
        assertEquals("main", dict.get("instance").getAsString());
    }

    // ----- fromDict (lenient) -----

    @Test
    void fromDictRoundTripsToDict() {
        MessageIdentity original = new MessageIdentity(multiLevelHier(), "opcua-adapter", "kep1");
        MessageIdentity parsed = MessageIdentity.fromDict(original.toDict());

        assertNotNull(parsed);
        assertEquals(original.getHier(), parsed.getHier());
        assertEquals(original.getPath(), parsed.getPath());
        assertEquals(original.getComponent(), parsed.getComponent());
        assertEquals(original.getInstance(), parsed.getInstance());
        assertEquals(original.getDevice(), parsed.getDevice());
    }

    @Test
    void fromDictMissingInstanceDefaultsToMain() {
        JsonObject src = new MessageIdentity(multiLevelHier(), "comp", "kep1").toDict();
        src.remove("instance");

        MessageIdentity parsed = MessageIdentity.fromDict(src);
        assertNotNull(parsed);
        assertEquals("main", parsed.getInstance());
    }

    @Test
    void fromDictMissingPathIsRecomputed() {
        JsonObject src = new MessageIdentity(multiLevelHier(), "comp", "main").toDict();
        src.remove("path");

        MessageIdentity parsed = MessageIdentity.fromDict(src);
        assertNotNull(parsed);
        assertEquals("dallas/zone-3/gw-01", parsed.getPath());
    }

    @Test
    void fromDictPresentPathIsTakenAsIs() {
        // The publisher is authoritative: a present path is never recomputed.
        JsonObject src = new MessageIdentity(multiLevelHier(), "comp", "main").toDict();
        src.addProperty("path", "publisher/authoritative/path");

        MessageIdentity parsed = MessageIdentity.fromDict(src);
        assertNotNull(parsed);
        assertEquals("publisher/authoritative/path", parsed.getPath());
    }

    @Test
    void fromDictMalformedHierYieldsNull() {
        // missing hier
        JsonObject noHier = new JsonObject();
        noHier.addProperty("component", "comp");
        assertNull(MessageIdentity.fromDict(noHier));

        // hier not an array
        JsonObject badHier = new JsonObject();
        badHier.addProperty("hier", "not-an-array");
        badHier.addProperty("component", "comp");
        assertNull(MessageIdentity.fromDict(badHier));

        // empty hier array
        JsonObject emptyHier = new JsonObject();
        emptyHier.add("hier", new JsonArray());
        emptyHier.addProperty("component", "comp");
        assertNull(MessageIdentity.fromDict(emptyHier));

        // hier entry not an object
        JsonObject primEntry = new JsonObject();
        JsonArray badEntries = new JsonArray();
        badEntries.add("just-a-string");
        primEntry.add("hier", badEntries);
        primEntry.addProperty("component", "comp");
        assertNull(MessageIdentity.fromDict(primEntry));

        // hier entry missing value
        JsonObject missingValue = new JsonObject();
        JsonArray entries = new JsonArray();
        JsonObject entry = new JsonObject();
        entry.addProperty("level", "device");
        entries.add(entry);
        missingValue.add("hier", entries);
        missingValue.addProperty("component", "comp");
        assertNull(MessageIdentity.fromDict(missingValue));
    }

    @Test
    void fromDictMissingComponentYieldsNull() {
        JsonObject src = new MessageIdentity(multiLevelHier(), "comp", "main").toDict();
        src.remove("component");
        assertNull(MessageIdentity.fromDict(src));
    }

    @Test
    void fromDictNullYieldsNull() {
        assertNull(MessageIdentity.fromDict(null));
    }

    @Test
    void toStringIsWireForm() {
        MessageIdentity id = new MessageIdentity(multiLevelHier(), "comp", "main");
        assertEquals(id.toDict().toString(), id.toString());
    }
}
