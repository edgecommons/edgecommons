/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;

/**
 * Unit tests for the {@link ParsedCommandLine} data holder and its {@link
 * ParsedCommandLine.Mode} enum.
 */
class ParsedCommandLineTest {

    @Test
    void defaultsAreNull() {
        ParsedCommandLine pcl = new ParsedCommandLine();
        assertNull(pcl.commandLine);
        assertNull(pcl.configArgs);
        assertNull(pcl.mode);
        assertNull(pcl.standaloneConfigPath);
        assertNull(pcl.thingName);
    }

    @Test
    void publicFieldsAreReadWrite() {
        ParsedCommandLine pcl = new ParsedCommandLine();
        String[] configArgs = {"FILE", "./config.json"};

        pcl.configArgs = configArgs;
        pcl.mode = ParsedCommandLine.Mode.STANDALONE;
        pcl.standaloneConfigPath = "./standalone-messaging.json";
        pcl.thingName = "my-thing";

        assertArrayEquals(configArgs, pcl.configArgs);
        assertEquals(ParsedCommandLine.Mode.STANDALONE, pcl.mode);
        assertEquals("./standalone-messaging.json", pcl.standaloneConfigPath);
        assertEquals("my-thing", pcl.thingName);
    }

    @Test
    void thingNameTakesFullStringValue() {
        // Guards against the historical bug that truncated the thing name to one char.
        ParsedCommandLine pcl = new ParsedCommandLine();
        pcl.thingName = "full-thing-name-value";
        assertEquals("full-thing-name-value", pcl.thingName);
    }

    @Test
    void modeEnumHasGreengrassAndStandalone() {
        assertEquals(2, ParsedCommandLine.Mode.values().length);
        assertEquals(ParsedCommandLine.Mode.GREENGRASS, ParsedCommandLine.Mode.valueOf("GREENGRASS"));
        assertEquals(ParsedCommandLine.Mode.STANDALONE, ParsedCommandLine.Mode.valueOf("STANDALONE"));
    }
}
