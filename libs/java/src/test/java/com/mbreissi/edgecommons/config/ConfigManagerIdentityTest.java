/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for {@link ConfigManager#getComponentIdentity()}: the once-at-construction UNS
 * identity resolution from the component's OWN config (top-level {@code hierarchy} +
 * {@code identity}), the zero-config {@code ["device"]} default, sanitization, and the
 * fail-fast startup errors.
 */
class ConfigManagerIdentityTest {

    private static JsonObject config(String json) {
        return JsonParser.parseString(json).getAsJsonObject();
    }

    private static ConfigManager manager(String thing, String json) {
        return new ConfigManager("com.test.TestComponent", "TestComponent", thing, null, config(json));
    }

    // ----- happy paths -----

    @Test
    void multiLevelHappyPathResolvesOrderedHierarchy() {
        ConfigManager cm = manager("gw-01", """
                {"component":{},
                 "hierarchy":{"levels":["site","factory","zone","device"]},
                 "identity":{"site":"dallas","factory":"finishing","zone":"zone-3"}}""");

        MessageIdentity id = cm.getComponentIdentity();
        assertNotNull(id);
        assertEquals(4, id.getHier().size());
        assertEquals(new MessageIdentity.HierEntry("site", "dallas"), id.getHier().get(0));
        assertEquals(new MessageIdentity.HierEntry("factory", "finishing"), id.getHier().get(1));
        assertEquals(new MessageIdentity.HierEntry("zone", "zone-3"), id.getHier().get(2));
        assertEquals(new MessageIdentity.HierEntry("device", "gw-01"), id.getHier().get(3));
        assertEquals("dallas/finishing/zone-3/gw-01", id.getPath());
        assertEquals("gw-01", id.getDevice());
        assertEquals("TestComponent", id.getComponent());
        assertNull(id.getInstance());   // D‑U28: component identity is component-scoped
    }

    @Test
    void zeroConfigDefaultsToSingleDeviceLevel() {
        ConfigManager cm = manager("thing-7", """
                {"component":{}}""");

        MessageIdentity id = cm.getComponentIdentity();
        assertNotNull(id);
        assertEquals(1, id.getHier().size());
        assertEquals(new MessageIdentity.HierEntry("device", "thing-7"), id.getHier().get(0));
        assertEquals("thing-7", id.getPath());
        assertEquals("thing-7", id.getDevice());
        assertEquals("TestComponent", id.getComponent());
        assertEquals("main", id.getInstance());
    }

    @Test
    void componentTokenOverridesPascalComponentName() {
        ConfigManager cm = manager("thing-7", """
                {"component":{"token":"opcua-adapter"}}""");

        MessageIdentity id = cm.getComponentIdentity();
        assertNotNull(id);
        assertEquals("opcua-adapter", id.getComponent());
    }

    @Test
    void identityValuesAndComponentAreSanitized() {
        // '/' and '+' are template-sanitizer blacklist characters -> '_'.
        ConfigManager cm = new ConfigManager("com.test.My#Comp", "My#Comp", "gw+01", null, config("""
                {"component":{},
                 "hierarchy":{"levels":["site","device"]},
                 "identity":{"site":"dal/las"}}"""));

        MessageIdentity id = cm.getComponentIdentity();
        assertNotNull(id);
        assertEquals("dal_las", id.getHier().get(0).value());
        assertEquals("gw_01", id.getDevice());
        assertEquals("My_Comp", id.getComponent());
        assertEquals("dal_las/gw_01", id.getPath());
    }

    // ----- fail-fast startup errors -----

    @Test
    void missingIdentityValueFailsFastNamingTheLevels() {
        IllegalStateException ex = assertThrows(IllegalStateException.class, () -> manager("gw-01", """
                {"component":{},
                 "hierarchy":{"levels":["site","zone","device"]},
                 "identity":{"site":"dallas"}}"""));
        assertTrue(ex.getMessage().contains("zone"), "error must name the missing level: " + ex.getMessage());
        assertTrue(ex.getMessage().contains("identity"));
    }

    @Test
    void identityKeyForTheDeviceLevelIsRejected() {
        IllegalStateException ex = assertThrows(IllegalStateException.class, () -> manager("gw-01", """
                {"component":{},
                 "hierarchy":{"levels":["site","device"]},
                 "identity":{"site":"dallas","device":"forged"}}"""));
        assertTrue(ex.getMessage().contains("device"));
        assertTrue(ex.getMessage().contains("thing name"));
    }

    @Test
    void identityKeyNotAmongLevelsIsRejected() {
        IllegalStateException ex = assertThrows(IllegalStateException.class, () -> manager("gw-01", """
                {"component":{},
                 "hierarchy":{"levels":["site","device"]},
                 "identity":{"site":"dallas","typoLevel":"x"}}"""));
        assertTrue(ex.getMessage().contains("typoLevel"));
    }

    @Test
    void invalidLevelNameIsRejected() {
        IllegalStateException ex = assertThrows(IllegalStateException.class, () -> manager("gw-01", """
                {"component":{},
                 "hierarchy":{"levels":["b@d","device"]}}"""));
        assertTrue(ex.getMessage().contains("b@d"));
    }

    @Test
    void duplicateLevelNamesAreRejected() {
        IllegalStateException ex = assertThrows(IllegalStateException.class, () -> manager("gw-01", """
                {"component":{},
                 "hierarchy":{"levels":["site","site","device"]}}"""));
        assertTrue(ex.getMessage().contains("duplicate"));
    }

    @Test
    void emptyLevelsArrayIsRejected() {
        assertThrows(IllegalStateException.class, () -> manager("gw-01", """
                {"component":{},
                 "hierarchy":{"levels":[]}}"""));
    }

    @Test
    void missingThingNameFailsFast() {
        IllegalStateException ex = assertThrows(IllegalStateException.class,
                () -> manager(null, """
                        {"component":{}}"""));
        assertTrue(ex.getMessage().contains("thing"));
    }

    @Test
    void malformedComponentTokenFailsFast() {
        IllegalStateException ex = assertThrows(IllegalStateException.class, () -> manager("gw-01", """
                {"component":{"token":""}}"""));
        assertTrue(ex.getMessage().contains("component.token"));
    }

    // ----- test/subclass bring-up -----

    @Test
    void protectedBringUpConstructorLeavesIdentityNull() {
        ConfigManager cm = new ConfigManager() { };
        assertNull(cm.getComponentIdentity());
    }

    // ----- end-to-end through the factory (schema validation + resolution) -----

    @Test
    void factoryPathValidatesAndResolvesNewTopLevelSections() throws Exception {
        // Proves the schema additions (top-level hierarchy/identity/topic + the new messaging
        // keys) pass the strict additionalProperties:false top level, and that the resolved
        // identity comes out of the real startup path.
        java.io.File tempFile = java.io.File.createTempFile("identity-config", ".json");
        tempFile.deleteOnExit();
        try (java.io.FileWriter writer = new java.io.FileWriter(tempFile)) {
            writer.write("""
                    {"component":{"global":{}},
                     "hierarchy":{"levels":["site","device"]},
                     "identity":{"site":"dallas"},
                     "topic":{"includeRoot":false},
                     "messaging":{"requestTimeoutSeconds":30}}""");
        }
        com.mbreissi.edgecommons.ParsedCommandLine cmdLine = new com.mbreissi.edgecommons.ParsedCommandLine();
        cmdLine.configArgs = new String[]{"FILE", tempFile.getAbsolutePath()};
        cmdLine.thingName = "gw-01";

        ConfigManager cm = ConfigManagerFactory.create("com.test.TestComponent", cmdLine);
        MessageIdentity id = cm.getComponentIdentity();
        assertNotNull(id);
        assertEquals("dallas/gw-01", id.getPath());
        assertEquals("TestComponent", id.getComponent());
        cm.close();
    }

    // ----- envelope shape of the resolved identity -----

    @Test
    void resolvedIdentitySerializesInCanonicalOrder() {
        ConfigManager cm = manager("gw-01", """
                {"component":{},
                 "hierarchy":{"levels":["site","device"]},
                 "identity":{"site":"dallas"}}""");

        JsonObject dict = cm.getComponentIdentity().toDict();
        assertEquals(List.of("hier", "path", "component", "instance"), List.copyOf(dict.keySet()));
        assertEquals("dallas/gw-01", dict.get("path").getAsString());
    }
}
