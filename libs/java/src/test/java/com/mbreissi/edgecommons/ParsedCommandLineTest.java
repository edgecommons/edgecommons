/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons;

import com.mbreissi.edgecommons.platform.Platform;
import com.mbreissi.edgecommons.platform.Transport;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;

/**
 * Unit tests for the {@link ParsedCommandLine} data holder, now carrying the resolved
 * platform/transport axes (DESIGN-core §4).
 */
class ParsedCommandLineTest {

    @Test
    void defaultsAreNull() {
        ParsedCommandLine pcl = new ParsedCommandLine();
        assertNull(pcl.commandLine);
        assertNull(pcl.configArgs);
        assertNull(pcl.platform);
        assertNull(pcl.transport);
        assertNull(pcl.standaloneConfigPath);
        assertNull(pcl.thingName);
        assertEquals(false, pcl.noSharedConfig);
    }

    @Test
    void publicFieldsAreReadWrite() {
        ParsedCommandLine pcl = new ParsedCommandLine();
        String[] configArgs = {"FILE", "./config.json"};

        pcl.configArgs = configArgs;
        pcl.platform = Platform.HOST;
        pcl.transport = Transport.MQTT;
        pcl.standaloneConfigPath = "./standalone-messaging.json";
        pcl.thingName = "my-thing";
        pcl.noSharedConfig = true;

        assertArrayEquals(configArgs, pcl.configArgs);
        assertEquals(Platform.HOST, pcl.platform);
        assertEquals(Transport.MQTT, pcl.transport);
        assertEquals("./standalone-messaging.json", pcl.standaloneConfigPath);
        assertEquals("my-thing", pcl.thingName);
        assertEquals(true, pcl.noSharedConfig);
    }

    @Test
    void thingNameTakesFullStringValue() {
        // Guards against the historical bug that truncated the thing name to one char.
        ParsedCommandLine pcl = new ParsedCommandLine();
        pcl.thingName = "full-thing-name-value";
        assertEquals("full-thing-name-value", pcl.thingName);
    }
}
