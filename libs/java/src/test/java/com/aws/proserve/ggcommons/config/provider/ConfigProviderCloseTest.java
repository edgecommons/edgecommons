/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.config.provider;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;

/**
 * Covers the {@link ConfigProvider#close()} default no-op (ConfigProvider L26).
 * The {@code ENV} provider does not override {@code close()}, so building one and
 * closing it exercises the base-class default. Closing must be a safe, idempotent
 * no-op (it releases no resources for the ENV source).
 */
class ConfigProviderCloseTest {

    @Test
    void envProviderCloseIsANoOp() {
        ConfigProvider provider = ConfigProviderBuilder.build(
                null, "com.example.Component", "test-thing",
                new String[]{"ENV", "SOME_VAR"}, null);

        // EnvironmentConfigProvider inherits ConfigProvider's no-op close().
        assertDoesNotThrow(provider::close);
        assertDoesNotThrow(provider::close); // idempotent
    }
}
