/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.config.provider;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for {@link ConfigProviderBuilder#build}. Covers the source-selection
 * switch: FILE and ENV (no MessagingClient required), the messaging-dependent
 * sources that must reject a null client with {@link IllegalStateException}, and
 * the unknown-source default that throws {@link IllegalArgumentException}.
 */
class ConfigProviderBuilderTest {

    @Test
    void buildFileProvider() {
        ConfigProvider provider = ConfigProviderBuilder.build(
                null, "com.test.Comp", "thing", new String[]{"FILE", "/tmp/config.json"}, null);
        assertNotNull(provider);
        assertTrue(provider.getConfigSource().contains("/tmp/config.json"));
        assertTrue(provider.getConfigSource().toLowerCase().contains("file"));
    }

    @Test
    void buildFileProviderLowerCaseSource() {
        // Source token is upper-cased before matching, so lower-case still works.
        ConfigProvider provider = ConfigProviderBuilder.build(
                null, "com.test.Comp", "thing", new String[]{"file", "/tmp/c.json"}, null);
        assertNotNull(provider);
    }

    @Test
    void buildConfigMapProvider() {
        ConfigProvider provider = ConfigProviderBuilder.build(
                null, "com.test.Comp", "thing", new String[]{"CONFIGMAP", "/etc/ggcommons", "config.json"}, null);
        try {
            assertNotNull(provider);
            assertTrue(provider.getConfigSource().contains("ConfigMap"));
            assertTrue(provider.getConfigSource().contains("ggcommons"));
        } finally {
            provider.close();
        }
    }

    @Test
    void buildConfigMapProviderUsesDefaults() {
        // No mount dir / key -> provider applies /etc/ggcommons + config.json defaults.
        ConfigProvider provider = ConfigProviderBuilder.build(
                null, "com.test.Comp", "thing", new String[]{"CONFIGMAP"}, null);
        try {
            assertNotNull(provider);
            assertTrue(provider.getConfigSource().contains("config.json"));
        } finally {
            provider.close();
        }
    }

    @Test
    void buildEnvProvider() {
        ConfigProvider provider = ConfigProviderBuilder.build(
                null, "com.test.Comp", "thing", new String[]{"ENV", "MY_CONFIG_VAR"}, null);
        assertNotNull(provider);
        assertTrue(provider.getConfigSource().contains("MY_CONFIG_VAR"));
    }

    @Test
    void buildEnvProviderDefaultVarName() {
        // No second arg -> defaults to "CONFIG".
        ConfigProvider provider = ConfigProviderBuilder.build(
                null, "com.test.Comp", "thing", new String[]{"ENV"}, null);
        assertNotNull(provider);
        assertTrue(provider.getConfigSource().contains("CONFIG"));
    }

    @Test
    void buildShadowWithoutMessagingClientThrows() {
        assertThrows(IllegalStateException.class, () -> ConfigProviderBuilder.build(
                null, "com.test.Comp", "thing", new String[]{"SHADOW"}, null));
    }

    @Test
    void buildGgConfigWithoutMessagingClientThrows() {
        assertThrows(IllegalStateException.class, () -> ConfigProviderBuilder.build(
                null, "com.test.Comp", "thing", new String[]{"GG_CONFIG"}, null));
    }

    @Test
    void buildConfigComponentWithoutMessagingClientThrows() {
        assertThrows(IllegalStateException.class, () -> ConfigProviderBuilder.build(
                null, "com.test.Comp", "thing", new String[]{"CONFIG_COMPONENT"}, null));
    }

    @Test
    void buildUnknownSourceThrows() {
        assertThrows(IllegalArgumentException.class, () -> ConfigProviderBuilder.build(
                null, "com.test.Comp", "thing", new String[]{"NOPE"}, null));
    }
}
