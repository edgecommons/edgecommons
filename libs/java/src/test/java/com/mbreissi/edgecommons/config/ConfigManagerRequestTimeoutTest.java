/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import java.time.Duration;

import static org.junit.jupiter.api.Assertions.assertEquals;

/**
 * Unit tests for the {@code messaging.requestTimeoutSeconds} config-model read
 * (UNS-CANONICAL-DESIGN §5 / D-U5): {@link ConfigManager#getMessagingRequestTimeout()} —
 * default 30 s, {@code 0} = disabled ({@code Duration.ZERO}), fractional seconds, lenient
 * parsing, and refresh on a config re-apply. EdgeCommons late-binds this value onto the
 * messaging client right after the ConfigManager is constructed (§1.5 init order).
 */
class ConfigManagerRequestTimeoutTest {

    private static JsonObject config(String json) {
        return JsonParser.parseString(json).getAsJsonObject();
    }

    private static ConfigManager manager(String json) {
        return new ConfigManager("com.test.TestComponent", "TestComponent", "gw-01", null, config(json));
    }

    @Test
    void defaultsToThirtySecondsWhenMessagingSectionIsAbsent() {
        assertEquals(Duration.ofSeconds(30), manager("""
                {"component":{}}""").getMessagingRequestTimeout());
    }

    @Test
    void defaultsWhenMessagingSectionHasNoTimeout() {
        assertEquals(Duration.ofSeconds(30), manager("""
                {"component":{}, "messaging":{"local":{"host":"h","port":1883,"clientId":"c"}}}""")
                .getMessagingRequestTimeout());
    }

    @Test
    void readsAnExplicitValue() {
        assertEquals(Duration.ofSeconds(12), manager("""
                {"component":{}, "messaging":{"requestTimeoutSeconds":12}}""")
                .getMessagingRequestTimeout());
    }

    @Test
    void zeroDisablesTheDefaultDeadline() {
        assertEquals(Duration.ZERO, manager("""
                {"component":{}, "messaging":{"requestTimeoutSeconds":0}}""")
                .getMessagingRequestTimeout());
    }

    @Test
    void fractionalSecondsAreSupported() {
        // The schema types the key as "number": 1.5 s -> 1500 ms.
        assertEquals(Duration.ofMillis(1500), manager("""
                {"component":{}, "messaging":{"requestTimeoutSeconds":1.5}}""")
                .getMessagingRequestTimeout());
    }

    @Test
    void lenientOnMalformedShapes() {
        // Non-object messaging / non-number value / negative (schema-rejected at startup anyway):
        // lenient default, like the other permissive subsystem sections.
        assertEquals(Duration.ofSeconds(30), manager("""
                {"component":{}, "messaging":"x"}""").getMessagingRequestTimeout());
        assertEquals(Duration.ofSeconds(30), manager("""
                {"component":{}, "messaging":{"requestTimeoutSeconds":"fast"}}""")
                .getMessagingRequestTimeout());
        assertEquals(Duration.ofSeconds(30), manager("""
                {"component":{}, "messaging":{"requestTimeoutSeconds":-5}}""")
                .getMessagingRequestTimeout());
    }

    @Test
    void reAppliedConfigRefreshesTheValue() {
        ConfigManager cm = manager("""
                {"component":{}}""");
        assertEquals(Duration.ofSeconds(30), cm.getMessagingRequestTimeout());
        cm.applyConfig(config("""
                {"component":{}, "messaging":{"requestTimeoutSeconds":5}}"""));
        assertEquals(Duration.ofSeconds(5), cm.getMessagingRequestTimeout());
    }

    @Test
    void protectedBringUpConstructorDefaultsToThirtySeconds() {
        assertEquals(Duration.ofSeconds(30), new ConfigManager() { }.getMessagingRequestTimeout());
    }
}
