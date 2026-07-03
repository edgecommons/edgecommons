/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.config;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

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
}
