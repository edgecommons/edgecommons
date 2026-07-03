/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.config;

import com.mbreissi.ggcommons.ParsedCommandLine;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import java.nio.file.Path;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Covers ConfigManager paths the existing {@code ConfigManagerTest} does not reach:
 *
 * <ul>
 *   <li>{@link ConfigManager#applyConfig(JsonObject)} <em>after</em> initialization completes,
 *       which takes the {@code !initializing} branch and calls {@code notifyConfigurationChanged()}
 *       (ConfigManager L106-107).</li>
 *   <li>{@code applyConfig} with a {@code component} block that has no {@code global} key,
 *       taking the "empty global" else branch (ConfigManager L104).</li>
 *   <li>{@link ConfigManager#notifyConfigurationChanged()} with a {@code null} listener present,
 *       which logs and skips it (ConfigManager L271-272).</li>
 * </ul>
 *
 * Built via the same FILE temp-config bring-up used by {@code ConfigManagerTest}.
 */
class ConfigManagerApplyConfigTest {

    private static final String INITIAL_CONFIG = """
            {\
            "logging":{"level":"INFO"},\
            "metricEmission":{"target":"log"},\
            "heartbeat":{"intervalSecs":30},\
            "tags":{},\
            "component":{"global":{"v":1}}\
            }""";

    private ConfigManager createConfigManager(String configPath) {
        try {
            ParsedCommandLine cmdLine = new ParsedCommandLine();
            cmdLine.configArgs = new String[]{"FILE", configPath};
            cmdLine.thingName = "test-thing";
            return ConfigManagerFactory.create("com.test.TestComponent", cmdLine);
        } catch (Exception e) {
            throw new RuntimeException("Failed to create ConfigManager: " + e.getMessage(), e);
        }
    }

    private File writeConfig(Path dir, String json) throws IOException {
        File f = File.createTempFile("cm-apply", ".json", dir.toFile());
        try (FileWriter w = new FileWriter(f)) {
            w.write(json);
            w.flush();
        }
        return f;
    }

    @Test
    void applyConfigAfterInitNotifiesListenersAndHandlesMissingGlobal(@TempDir Path tempDir) throws IOException {
        ConfigManager cm = createConfigManager(writeConfig(tempDir, INITIAL_CONFIG).getAbsolutePath());
        cm.completeInitialization(); // flips the internal "initializing" flag to false

        boolean[] notified = {false};
        cm.addConfigChangeListener(() -> { notified[0] = true; return true; });

        // New config whose "component" has NO "global" -> exercises the empty-global else branch,
        // and because initialization is complete, applyConfig notifies the listener. (Must be
        // schema-valid: hot reloads are now re-validated, so use instances:[] rather than an
        // unknown "component" property.)
        JsonObject reload = JsonParser.parseString("""
                {"component":{"instances":[]}}""").getAsJsonObject();
        cm.applyConfig(reload);

        assertTrue(notified[0], "applyConfig after init must notify configuration-change listeners");
        assertNotNull(cm.getGlobalConfig(), "missing global must yield an empty (non-null) global config");
        assertEquals(0, cm.getGlobalConfig().size(), "global config should be empty when absent");
    }

    @Test
    void applyConfigRejectsInvalidHotReloadAndKeepsPrevious(@TempDir Path tempDir) throws IOException {
        ConfigManager cm = createConfigManager(writeConfig(tempDir, INITIAL_CONFIG).getAbsolutePath());
        cm.completeInitialization();

        boolean[] notified = {false};
        cm.addConfigChangeListener(() -> { notified[0] = true; return true; });

        // Invalid hot reload (unknown top-level section under additionalProperties:false) must be
        // rejected; the previous configuration is retained and listeners are NOT notified
        // (parity with Python/Rust/TS).
        JsonObject bad = JsonParser.parseString("""
                {"component":{"global":{"v":1}},"bogusSection":true}""").getAsJsonObject();
        cm.applyConfig(bad);

        assertFalse(notified[0], "an invalid hot reload must not notify listeners");
        assertEquals(1, cm.getGlobalConfig().get("v").getAsInt(),
                "the previous global config must be retained after a rejected reload");
    }

    @Test
    void applyConfigRefreshesTheFullConfigSnapshot(@TempDir Path tempDir) throws IOException {
        ConfigManager cm = createConfigManager(writeConfig(tempDir, INITIAL_CONFIG).getAbsolutePath());
        cm.completeInitialization();

        // getFullConfig() must reflect the APPLIED configuration after a hot reload / push -
        // the effective-config publisher and the get-configuration verb read it.
        JsonObject reload = JsonParser.parseString("""
                {"component":{"global":{"v":2}}}""").getAsJsonObject();
        cm.applyConfig(reload);
        assertEquals(2, cm.getFullConfig().getAsJsonObject("component")
                        .getAsJsonObject("global").get("v").getAsInt(),
                "getFullConfig() must return the applied config, not the startup snapshot");

        // A rejected (schema-invalid) reload must NOT touch the snapshot.
        JsonObject bad = JsonParser.parseString("""
                {"component":{"global":{"v":3}},"bogusSection":true}""").getAsJsonObject();
        cm.applyConfig(bad);
        assertEquals(2, cm.getFullConfig().getAsJsonObject("component")
                        .getAsJsonObject("global").get("v").getAsInt(),
                "a rejected reload must keep the previous full-config snapshot");
    }

    // ----- reloadFromProvider (the reload-config verb's action, DESIGN-uns §9.5) -----

    @Test
    void reloadFromProviderReFetchesAppliesAndNotifies(@TempDir Path tempDir) throws IOException {
        File configFile = writeConfig(tempDir, INITIAL_CONFIG);
        ConfigManager cm = createConfigManager(configFile.getAbsolutePath());
        cm.completeInitialization();

        boolean[] notified = {false};
        cm.addConfigChangeListener(() -> { notified[0] = true; return true; });

        // Change the file on disk, then reload-config: the provider re-reads and re-applies.
        try (FileWriter w = new FileWriter(configFile)) {
            w.write("""
                    {"logging":{"level":"DEBUG"},"component":{"global":{"v":7}}}""");
            w.flush();
        }
        assertTrue(cm.reloadFromProvider(), "a valid re-fetch must be applied and ack'd");
        assertTrue(notified[0], "a successful reload must notify configuration-change listeners");
        assertEquals(7, cm.getGlobalConfig().get("v").getAsInt());
        assertEquals(7, cm.getFullConfig().getAsJsonObject("component")
                        .getAsJsonObject("global").get("v").getAsInt(),
                "the full-config snapshot must be the reloaded document");
    }

    @Test
    void reloadFromProviderRejectsAnInvalidDocumentAndKeepsPrevious(@TempDir Path tempDir)
            throws IOException {
        File configFile = writeConfig(tempDir, INITIAL_CONFIG);
        ConfigManager cm = createConfigManager(configFile.getAbsolutePath());
        cm.completeInitialization();

        boolean[] notified = {false};
        cm.addConfigChangeListener(() -> { notified[0] = true; return true; });

        try (FileWriter w = new FileWriter(configFile)) {
            w.write("""
                    {"component":{"global":{"v":9}},"bogusSection":true}""");
            w.flush();
        }
        assertFalse(cm.reloadFromProvider(), "a schema-invalid re-fetch must be rejected");
        assertFalse(notified[0], "a rejected reload must not notify listeners");
        assertEquals(1, cm.getGlobalConfig().get("v").getAsInt(),
                "the previous configuration must be retained");
    }

    @Test
    void reloadFromProviderReportsFalseWhenTheFetchFails(@TempDir Path tempDir) throws IOException {
        File configFile = writeConfig(tempDir, INITIAL_CONFIG);
        ConfigManager cm = createConfigManager(configFile.getAbsolutePath());
        cm.completeInitialization();

        assertTrue(configFile.delete(), "test setup: remove the config file");
        assertFalse(cm.reloadFromProvider(),
                "a failing re-fetch must be reported, never thrown");
        assertEquals(1, cm.getGlobalConfig().get("v").getAsInt(),
                "the previous configuration must be retained");
    }

    @Test
    void reloadFromProviderWithoutAProviderIsFalse() {
        // The mock/subclass bring-up case: no provider wired.
        assertFalse(new com.mbreissi.ggcommons.test.MockConfigurationService().reloadFromProvider());
    }

    @Test
    void notifyToleratesNullListener(@TempDir Path tempDir) throws IOException {
        ConfigManager cm = createConfigManager(writeConfig(tempDir, INITIAL_CONFIG).getAbsolutePath());

        boolean[] realCalled = {false};
        cm.addConfigChangeListener(null); // null listener must be skipped, not crash
        cm.addConfigChangeListener(() -> { realCalled[0] = true; return true; });

        cm.notifyConfigurationChanged();

        assertTrue(realCalled[0], "a null listener must be skipped while real listeners still fire");
    }
}
