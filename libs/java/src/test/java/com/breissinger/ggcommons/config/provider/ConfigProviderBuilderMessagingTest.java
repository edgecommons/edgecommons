/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.config.provider;

import com.breissinger.ggcommons.messaging.MessagingClient;
import com.breissinger.ggcommons.test.MockConfigurationService;
import com.breissinger.ggcommons.test.MockMessagingService;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Covers the {@link ConfigProviderBuilder#build} <em>success</em> branches that require a
 * non-null {@link MessagingClient}, which the existing {@code ConfigProviderBuilderTest}
 * only exercises in their null-client throwing form:
 *
 * <ul>
 *   <li>{@code GG_CONFIG} -> {@code new GreengrassConfigProvider(...)} (builder L45-46)</li>
 *   <li>{@code CONFIG_COMPONENT} -> {@code new ConfigComponentProvider(...)} (builder L52-53)</li>
 * </ul>
 *
 * Both provider constructors do only cheap, broker-free work
 * (GreengrassConfigProvider stores fields; ConfigComponentProvider resolves topic templates
 * and registers a no-op subscription on the mock client), so no Nucleus/AWS is needed.
 * The {@code SHADOW} success path is intentionally not covered here because its constructor
 * casts the native client to a real IPC client (requires a live Nucleus).
 */
class ConfigProviderBuilderMessagingTest {

    @Test
    void buildGgConfigProviderWithMessagingClient() {
        MockMessagingService messaging = new MockMessagingService();
        ConfigProvider provider = ConfigProviderBuilder.build(
                new MockConfigurationService(), "com.test.Comp", "thing",
                new String[]{"GG_CONFIG", "some.config.Component", "ComponentConfig"}, messaging);

        assertNotNull(provider);
        assertTrue(provider instanceof GreengrassConfigProvider);
        assertNotNull(provider.getConfigSource());
    }

    @Test
    void buildGgConfigProviderDefaultsWithMessagingClient() {
        // No component-name / key args -> the null-default branches in the builder are taken.
        MockMessagingService messaging = new MockMessagingService();
        ConfigProvider provider = ConfigProviderBuilder.build(
                new MockConfigurationService(), "com.test.Comp", "thing",
                new String[]{"GG_CONFIG"}, messaging);

        assertNotNull(provider);
        assertTrue(provider instanceof GreengrassConfigProvider);
    }

    @Test
    void buildConfigComponentProviderWithMessagingClient() {
        MockMessagingService messaging = new MockMessagingService();
        ConfigProvider provider = ConfigProviderBuilder.build(
                new MockConfigurationService(), "com.test.Comp", "thing",
                new String[]{"CONFIG_COMPONENT"}, messaging);

        assertNotNull(provider);
        assertTrue(provider instanceof ConfigComponentProvider);
        assertNotNull(provider.getConfigSource());
    }
}
