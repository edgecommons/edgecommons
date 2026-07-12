/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

import com.mbreissi.edgecommons.ParsedCommandLine;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import java.time.Duration;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.atomic.AtomicBoolean;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests targeting {@link ConfigManagerFactory#create} branches not exercised by
 * {@code ConfigManagerTest}: short-vs-full component-name parsing, thing-name resolution
 * from the command line, and the error paths (no config / schema validation failure).
 */
class ConfigManagerFactoryTest {

    private static File writeTempConfig(String json) throws IOException {
        File tempFile = File.createTempFile("factory-config", ".json");
        tempFile.deleteOnExit();
        try (FileWriter writer = new FileWriter(tempFile)) {
            writer.write(json);
            writer.flush();
        }
        return tempFile;
    }

    private static ParsedCommandLine fileCmdLine(File config, String thing) {
        ParsedCommandLine cmdLine = new ParsedCommandLine();
        cmdLine.configArgs = new String[]{"FILE", config.getAbsolutePath()};
        cmdLine.thingName = thing;
        return cmdLine;
    }

    private static final String VALID_CONFIG = """
            {"logging":{"level":"INFO"},\
            "metricEmission":{"target":"log"},\
            "heartbeat":{"intervalSecs":30},\
            "tags":{},\
            "component":{"global":{}}}""";

    @Test
    void createParsesDottedComponentNameIntoShortName() throws Exception {
        File config = writeTempConfig(VALID_CONFIG);
        ConfigManager cm = ConfigManagerFactory.create("com.example.MyComponent",
                fileCmdLine(config, "my-thing"));
        assertEquals("com.example.MyComponent", cm.getComponentFullName());
        assertEquals("MyComponent", cm.getComponentName());
        assertEquals("my-thing", cm.getThingName());
    }

    @Test
    void createHandlesComponentNameWithoutDot() throws Exception {
        File config = writeTempConfig(VALID_CONFIG);
        ConfigManager cm = ConfigManagerFactory.create("FlatName",
                fileCmdLine(config, "thing-x"));
        // No '.' -> short name equals full name.
        assertEquals("FlatName", cm.getComponentFullName());
        assertEquals("FlatName", cm.getComponentName());
    }

    @Test
    void createThrowsConfigurationExceptionForInvalidSchema() throws Exception {
        // Missing required "component" block -> schema validation failure.
        File config = writeTempConfig("""
                {"logging":{"level":"INFO"}}""");
        ConfigurationException ex = assertThrows(ConfigurationException.class,
                () -> ConfigManagerFactory.create("com.test.Comp", fileCmdLine(config, "t")));
        assertTrue(ex.getMessage().toLowerCase().contains("validation"));
    }

    @Test
    void schemaValidationRunsBeforeInitialComponentValidators() throws Exception {
        File config = writeTempConfig("""
                {"logging":{"level":"INFO"}}""");
        AtomicBoolean validatorCalled = new AtomicBoolean();

        assertThrows(ConfigurationException.class, () -> ConfigManagerFactory.create(
                "com.test.Comp", fileCmdLine(config, "t"), null,
                Map.of("camera", (candidate, current, phase) -> {
                    validatorCalled.set(true);
                    return ConfigurationCandidateValidator.Result.accept();
                }), Duration.ofSeconds(1)));

        assertFalse(validatorCalled.get(),
                "schema-invalid candidates must never reach component validators");
    }

    @Test
    void registeredValidatorsRunOnTheInitialCandidateUnderTheirRegisteredNames() throws Exception {
        File config = writeTempConfig(VALID_CONFIG);
        List<String> ran = new CopyOnWriteArrayList<>();
        Map<String, ConfigurationCandidateValidator> validators = new LinkedHashMap<>();
        // The full legal name shape: alphanumerics plus '.', '_' and '-'.
        validators.put("camera", (candidate, current, phase) -> {
            ran.add("camera:" + phase);
            return ConfigurationCandidateValidator.Result.accept();
        });
        validators.put("sb.probe_1-a", (candidate, current, phase) -> {
            ran.add("sb.probe_1-a:" + phase);
            return ConfigurationCandidateValidator.Result.accept();
        });

        ConfigManager cm = ConfigManagerFactory.create("com.test.Comp",
                fileCmdLine(config, "t"), null, validators, Duration.ofSeconds(2));

        assertEquals(2, ran.size(), "every registered validator sees the initial candidate");
        assertTrue(ran.contains("camera:INITIAL"));
        assertTrue(ran.contains("sb.probe_1-a:INITIAL"));
        assertEquals(1, cm.getConfigGeneration());
    }

    @Test
    void aMalformedValidatorRegistrationFailsComponentStartup() throws Exception {
        File config = writeTempConfig(VALID_CONFIG);
        ConfigurationCandidateValidator accepts = (candidate, current, phase) ->
                ConfigurationCandidateValidator.Result.accept();

        // Validator names become operator-facing error identifiers, so the shape is enforced up
        // front rather than producing an unusable diagnostic at rejection time.
        for (String illegalName : new String[]{"", "-leading", "has space", "a".repeat(65)}) {
            Map<String, ConfigurationCandidateValidator> named = new HashMap<>();
            named.put(illegalName, accepts);
            ConfigurationException ex = assertThrows(ConfigurationException.class,
                    () -> ConfigManagerFactory.create("com.test.Comp", fileCmdLine(config, "t"),
                            null, named, Duration.ofSeconds(2)),
                    "illegal validator name accepted: '" + illegalName + "'");
            assertInstanceOf(IllegalArgumentException.class, ex.getCause());
        }

        Map<String, ConfigurationCandidateValidator> nullName = new HashMap<>();
        nullName.put(null, accepts);
        assertInstanceOf(IllegalArgumentException.class,
                assertThrows(ConfigurationException.class,
                        () -> ConfigManagerFactory.create("com.test.Comp", fileCmdLine(config, "t"),
                                null, nullName, Duration.ofSeconds(2))).getCause());

        Map<String, ConfigurationCandidateValidator> nullValidator = new HashMap<>();
        nullValidator.put("camera", null);
        assertInstanceOf(NullPointerException.class,
                assertThrows(ConfigurationException.class,
                        () -> ConfigManagerFactory.create("com.test.Comp", fileCmdLine(config, "t"),
                                null, nullValidator, Duration.ofSeconds(2))).getCause());
    }

    @Test
    void aSchemaInvalidReloadIsRefusedAndTheRunningConfigurationIsUntouched() throws Exception {
        File config = writeTempConfig(VALID_CONFIG);
        ConfigManager cm = ConfigManagerFactory.create("com.test.Comp",
                fileCmdLine(config, "my-thing"));
        JsonObject running = cm.getFullConfig();

        // The operator edits the file into a schema-invalid state and asks for a reload.
        try (FileWriter writer = new FileWriter(config)) {
            writer.write("""
                    {"component":{"global":{}},"notASection":true}""");
        }

        assertFalse(cm.reloadFromProvider(), "reload-config must report the refusal");
        assertEquals(running, cm.getFullConfig(),
                "the running configuration survives a bad reload byte for byte");
        assertEquals(1, cm.getConfigGeneration(), "a refused reload produces no new generation");
    }

    @Test
    void createThrowsConfigurationExceptionForMissingFile() {
        // Nonexistent file -> FileConfigProvider.loadConfiguration throws, wrapped as ConfigurationException.
        ParsedCommandLine cmdLine = new ParsedCommandLine();
        cmdLine.configArgs = new String[]{"FILE", "/path/does/not/exist-" + System.nanoTime() + ".json"};
        cmdLine.thingName = "t";
        assertThrows(ConfigurationException.class,
                () -> ConfigManagerFactory.create("com.test.Comp", cmdLine));
    }
}
