/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.config.provider;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for {@link EnvironmentConfigProvider} (package-private), constructed via
 * {@link ConfigProviderBuilder} with the {@code ENV} source. Covers the
 * "environment variable not defined" RuntimeException path and {@code getConfigSource()}.
 *
 * <p>The ENV provider does not require a {@code ConfigManager} or {@code MessagingClient}
 * during construction or {@code loadConfiguration()}, so both are passed as {@code null}.
 */
class EnvironmentConfigProviderTest {

    /** A variable name that is extremely unlikely to be defined in any environment. */
    private static final String UNDEFINED_VAR =
            "GGCOMMONS_DEFINITELY_NOT_DEFINED_ENV_VAR_" + System.nanoTime();

    private ConfigProvider buildEnvProvider(String varName) {
        return ConfigProviderBuilder.build(
                null,                         // configManager (unused by ENV provider)
                "com.example.Component",      // componentName
                "test-thing",                 // thingName
                new String[]{"ENV", varName}, // configArgs -> ENV source
                null);                        // messagingClient (unused by ENV provider)
    }

    @Test
    void loadConfigurationThrowsWhenVariableNotDefined() {
        ConfigProvider provider = buildEnvProvider(UNDEFINED_VAR);

        RuntimeException ex = assertThrows(RuntimeException.class, provider::loadConfiguration);
        assertTrue(ex.getMessage().contains(UNDEFINED_VAR),
                "exception message should name the missing variable");
    }

    @Test
    void getConfigSourceDescribesEnvironmentVariable() {
        ConfigProvider provider = buildEnvProvider("MY_VAR");

        String source = provider.getConfigSource();
        assertNotNull(source);
        assertTrue(source.startsWith("Environment"));
        assertTrue(source.contains("MY_VAR"));
    }

    @Test
    void defaultEnvVarNameIsUsedWhenNotProvided() {
        // configArgs with only "ENV" -> provider defaults to the "CONFIG" env var name.
        ConfigProvider provider = ConfigProviderBuilder.build(
                null, "com.example.Component", "test-thing",
                new String[]{"ENV"}, null);

        assertTrue(provider.getConfigSource().contains("CONFIG"));
    }
}
