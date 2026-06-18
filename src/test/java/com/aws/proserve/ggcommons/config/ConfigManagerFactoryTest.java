/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.config;

import com.aws.proserve.ggcommons.ParsedCommandLine;
import org.junit.jupiter.api.Test;

import java.io.File;
import java.io.FileWriter;
import java.io.IOException;

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

    private static final String VALID_CONFIG =
            "{\"logging\":{\"level\":\"INFO\"}," +
            "\"metricEmission\":{\"target\":\"log\"}," +
            "\"heartbeat\":{\"intervalSecs\":30}," +
            "\"tags\":{}," +
            "\"component\":{\"global\":{}}}";

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
        File config = writeTempConfig("{\"logging\":{\"level\":\"INFO\"}}");
        ConfigurationException ex = assertThrows(ConfigurationException.class,
                () -> ConfigManagerFactory.create("com.test.Comp", fileCmdLine(config, "t")));
        assertTrue(ex.getMessage().toLowerCase().contains("validation"));
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
