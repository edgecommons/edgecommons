/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.config.provider;

import com.mbreissi.ggcommons.config.ConfigManager;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;
import org.mockito.ArgumentCaptor;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

/**
 * Unit tests for {@link FileConfigProvider} (package-private). Covers the
 * {@code loadConfiguration()} success and error paths and, most importantly, the
 * {@code onChange()} reload callback (L77-L80) that the existing provider tests
 * leave uncovered.
 *
 * <p>The provider starts a {@link com.mbreissi.ggcommons.utils.FileWatcher} daemon
 * thread in its constructor; every test {@code close()}s the provider so no thread leaks.
 * The {@link ConfigManager} is a Mockito mock so {@code applyConfig(...)} can be verified
 * without exercising the real schema validator or logging reconfiguration.
 */
class FileConfigProviderTest {

    private static Path writeConfig(String json) throws IOException {
        Path file = Files.createTempFile("ggcommons-file-config", ".json");
        Files.write(file, json.getBytes(StandardCharsets.UTF_8));
        file.toFile().deleteOnExit();
        return file;
    }

    @Test
    void loadConfigurationReadsAndParsesFile() throws IOException {
        ConfigManager cm = mock(ConfigManager.class);
        Path file = writeConfig("{\"component\":{\"name\":\"x\"},\"foo\":42}");

        FileConfigProvider provider = new FileConfigProvider(cm, file.toString());
        try {
            JsonObject loaded = provider.loadConfiguration();
            assertNotNull(loaded);
            assertEquals(42, loaded.get("foo").getAsInt());
            assertTrue(provider.getConfigSource().contains(file.toString()));
        } finally {
            provider.close();
        }
    }

    @Test
    void loadConfigurationThrowsRuntimeExceptionForMissingFile() {
        ConfigManager cm = mock(ConfigManager.class);
        String missing = "this-file-does-not-exist-" + System.nanoTime() + ".json";

        FileConfigProvider provider = new FileConfigProvider(cm, missing);
        try {
            RuntimeException ex = assertThrows(RuntimeException.class, provider::loadConfiguration);
            assertTrue(ex.getMessage().contains(missing),
                    "exception message should name the unreadable file");
        } finally {
            provider.close();
        }
    }

    @Test
    void loadConfigurationThrowsRuntimeExceptionForMalformedJson() throws IOException {
        ConfigManager cm = mock(ConfigManager.class);
        Path file = writeConfig("{ this is : not valid json ]");

        FileConfigProvider provider = new FileConfigProvider(cm, file.toString());
        try {
            assertThrows(RuntimeException.class, provider::loadConfiguration);
        } finally {
            provider.close();
        }
    }

    @Test
    void onChangeReloadsAndAppliesConfigToParentManager() throws IOException {
        ConfigManager cm = mock(ConfigManager.class);
        Path file = writeConfig("{\"component\":{\"name\":\"reloaded\"},\"version\":2}");

        FileConfigProvider provider = new FileConfigProvider(cm, file.toString());
        try {
            // Directly invoke the FileWatcher callback rather than racing the OS watcher.
            provider.onChange();

            ArgumentCaptor<JsonObject> captor = ArgumentCaptor.forClass(JsonObject.class);
            verify(cm).applyConfig(captor.capture());
            JsonObject applied = captor.getValue();
            assertNotNull(applied);
            assertEquals(2, applied.get("version").getAsInt());
        } finally {
            provider.close();
        }
    }
}
