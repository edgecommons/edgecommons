/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.config.provider;

import org.junit.jupiter.api.Assumptions;
import org.junit.jupiter.api.Test;

import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Covers the {@link EnvironmentConfigProvider#loadConfiguration()} path where the
 * environment variable <em>is</em> defined. The existing {@code EnvironmentConfigProviderTest}
 * only covers the "variable not defined" branch.
 *
 * <p>Java cannot set process environment variables portably from a test, so we pick an
 * environment variable that already exists in this JVM and whose value is not a JSON object
 * (e.g. {@code PATH}). Loading it drives the "configStr != null" branch and then the
 * {@link com.google.gson.JsonSyntaxException} catch branch
 * (EnvironmentConfigProvider L35, L37, L39-41).
 */
class EnvironmentConfigProviderDefinedTest {

    /** Finds any defined env var whose value is clearly not a JSON object. */
    private static String findNonJsonEnvVar() {
        for (Map.Entry<String, String> e : System.getenv().entrySet()) {
            String value = e.getValue();
            if (value == null) continue;
            String trimmed = value.trim();
            // We want a value that gson cannot parse into a JsonObject so the catch fires.
            if (!trimmed.isEmpty() && !trimmed.startsWith("{")) {
                return e.getKey();
            }
        }
        return null;
    }

    @Test
    void loadConfigurationThrowsOnNonJsonDefinedVariable() {
        String varName = findNonJsonEnvVar();
        Assumptions.assumeTrue(varName != null,
                "no defined environment variable with a non-JSON value is available");

        ConfigProvider provider = ConfigProviderBuilder.build(
                null, "com.example.Component", "test-thing",
                new String[]{"ENV", varName}, null);

        // The variable is defined but its value is not valid JSON: the JsonSyntaxException
        // is wrapped and rethrown as a RuntimeException naming the variable.
        RuntimeException ex = assertThrows(RuntimeException.class, provider::loadConfiguration);
        assertTrue(ex.getMessage().contains(varName),
                "exception message should name the offending variable");
    }
}
