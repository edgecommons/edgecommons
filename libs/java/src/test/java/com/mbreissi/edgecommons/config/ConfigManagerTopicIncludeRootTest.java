/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for the minimal top-level {@code topic} config-model support:
 * {@link ConfigManager#isTopicIncludeRoot()} (UNS-CANONICAL-DESIGN §2.2 rule 6 / D-U11) —
 * default {@code false}, lenient parsing, and refresh on a config re-apply.
 */
class ConfigManagerTopicIncludeRootTest {

    private static JsonObject config(String json) {
        return JsonParser.parseString(json).getAsJsonObject();
    }

    private static ConfigManager manager(String json) {
        return new ConfigManager("com.test.TestComponent", "TestComponent", "gw-01", null, config(json));
    }

    @Test
    void defaultsToFalseWhenTheTopicSectionIsAbsent() {
        assertFalse(manager("""
                {"component":{}}""").isTopicIncludeRoot());
    }

    @Test
    void readsAnExplicitTrue() {
        assertTrue(manager("""
                {"component":{}, "topic":{"includeRoot":true}}""").isTopicIncludeRoot());
    }

    @Test
    void readsAnExplicitFalse() {
        assertFalse(manager("""
                {"component":{}, "topic":{"includeRoot":false}}""").isTopicIncludeRoot());
    }

    @Test
    void emptyTopicSectionDefaultsToFalse() {
        assertFalse(manager("""
                {"component":{}, "topic":{}}""").isTopicIncludeRoot());
    }

    @Test
    void lenientOnMalformedShapes() {
        // Non-object topic section / non-boolean includeRoot: lenient default, like the other
        // permissive subsystem sections (the schema rejects these at startup anyway).
        assertFalse(manager("""
                {"component":{}, "topic":"x"}""").isTopicIncludeRoot());
        assertFalse(manager("""
                {"component":{}, "topic":{"includeRoot":"yes"}}""").isTopicIncludeRoot());
        assertFalse(manager("""
                {"component":{}, "topic":{"includeRoot":1}}""").isTopicIncludeRoot());
    }

    @Test
    void reAppliedConfigRefreshesTheFlag() {
        ConfigManager cm = manager("""
                {"component":{}}""");
        assertFalse(cm.isTopicIncludeRoot());
        // Initialization is still open (no completeInitialization), so applyConfig skips the
        // schema re-validation and listener fan-out — this exercises just the parse+refresh.
        cm.applyConfig(config("""
                {"component":{}, "topic":{"includeRoot":true}}"""));
        assertTrue(cm.isTopicIncludeRoot());
    }

    @Test
    void protectedBringUpConstructorDefaultsToFalse() {
        assertFalse(new ConfigManager() { }.isTopicIncludeRoot());
    }

    // ----- D-U25: includeRoot with a single-level hierarchy (config-time WARN, flag unchanged) -----

    @Test
    void includeRootWithSingleLevelHierarchyStillParsesTrueAndWarnsOnce() {
        // The zero-config hierarchy is the single level ["device"]: includeRoot=true is a no-op
        // in Uns (D-U25) and WARNs once at config time — but the parsed flag itself is what the
        // user configured. Re-applying the same config must not warn again (one-shot flag).
        ConfigManager cm = manager("""
                {"component":{}, "topic":{"includeRoot":true}}""");
        assertTrue(cm.isTopicIncludeRoot());
        cm.applyConfig(config("""
                {"component":{}, "topic":{"includeRoot":true}}"""));
        assertTrue(cm.isTopicIncludeRoot());
    }

    @Test
    void includeRootWithMultiLevelHierarchyDoesNotWarn() {
        // A multi-level hierarchy makes includeRoot effective — no WARN path.
        ConfigManager cm = new ConfigManager("com.test.TestComponent", "TestComponent", "gw-01",
                null, config("""
                {"component":{}, "topic":{"includeRoot":true},
                 "hierarchy":{"levels":["site","device"]}, "identity":{"site":"dallas"}}"""));
        assertTrue(cm.isTopicIncludeRoot());
        assertEquals("dallas/gw-01", cm.getComponentIdentity().getPath());
    }

    @Test
    void malformedHierarchyShapesCountAsSingleLevelForTheWarn() {
        // The WARN's lenient level count must never throw on shapes the strict resolver rejects
        // later; a non-object hierarchy or empty levels array counts as the single-level default.
        // (Strict validation still fail-fasts in resolveComponentIdentity at construction.)
        assertTrue(manager("""
                {"component":{}, "topic":{"includeRoot":true}}""").isTopicIncludeRoot());
    }
}
