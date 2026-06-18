/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons;

import org.apache.commons.cli.Options;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for the static {@link GGCommons#processArgs(String, String[], Options)}
 * argument parser. Covers defaults, the explicit thing/mode flags, STANDALONE with a
 * path, and the IllegalArgumentException error branches. The --help/-h branch is
 * deliberately NOT exercised because it calls {@code System.exit(0)}.
 */
class GGCommonsProcessArgsTest {

    private static final String COMPONENT = "com.example.TestComponent";

    @Test
    void defaultsWhenNoConfigOrModeSpecified() {
        ParsedCommandLine pcl = GGCommons.processArgs(COMPONENT, new String[]{}, null);

        assertNotNull(pcl);
        // -c absent -> default GG_CONFIG
        assertArrayEquals(new String[]{"GG_CONFIG"}, pcl.configArgs);
        // -m absent -> default GREENGRASS
        assertEquals(ParsedCommandLine.Mode.GREENGRASS, pcl.mode);
        assertNull(pcl.standaloneConfigPath);
        assertNull(pcl.thingName);
        assertNotNull(pcl.commandLine);
    }

    @Test
    void explicitThingTakesFullStringValue() {
        // Guards against the historical bug that truncated the thing name to one char.
        ParsedCommandLine pcl = GGCommons.processArgs(
                COMPONENT, new String[]{"-t", "my-full-thing-name"}, null);

        assertEquals("my-full-thing-name", pcl.thingName);
    }

    @Test
    void longThingOptionTakesFullStringValue() {
        ParsedCommandLine pcl = GGCommons.processArgs(
                COMPONENT, new String[]{"--thing", "another-full-thing"}, null);

        assertEquals("another-full-thing", pcl.thingName);
    }

    @Test
    void explicitConfigSourceWithArgs() {
        ParsedCommandLine pcl = GGCommons.processArgs(
                COMPONENT, new String[]{"-c", "FILE", "./config.json"}, null);

        assertArrayEquals(new String[]{"FILE", "./config.json"}, pcl.configArgs);
        // mode still defaults to GREENGRASS
        assertEquals(ParsedCommandLine.Mode.GREENGRASS, pcl.mode);
    }

    @Test
    void greengrassModeExplicit() {
        ParsedCommandLine pcl = GGCommons.processArgs(
                COMPONENT, new String[]{"-m", "GREENGRASS"}, null);

        assertEquals(ParsedCommandLine.Mode.GREENGRASS, pcl.mode);
        assertNull(pcl.standaloneConfigPath);
    }

    @Test
    void standaloneModeWithPathSetsStandaloneConfigPath() {
        ParsedCommandLine pcl = GGCommons.processArgs(
                COMPONENT,
                new String[]{"-m", "STANDALONE", "./standalone-messaging.json"},
                null);

        assertEquals(ParsedCommandLine.Mode.STANDALONE, pcl.mode);
        assertEquals("./standalone-messaging.json", pcl.standaloneConfigPath);
    }

    @Test
    void standaloneModeIsCaseInsensitive() {
        ParsedCommandLine pcl = GGCommons.processArgs(
                COMPONENT,
                new String[]{"-m", "standalone", "./msg.json"},
                null);

        assertEquals(ParsedCommandLine.Mode.STANDALONE, pcl.mode);
        assertEquals("./msg.json", pcl.standaloneConfigPath);
    }

    @Test
    void standaloneWithoutPathThrows() {
        assertThrows(IllegalArgumentException.class, () ->
                GGCommons.processArgs(COMPONENT, new String[]{"-m", "STANDALONE"}, null));
    }

    @Test
    void unknownModeThrows() {
        assertThrows(IllegalArgumentException.class, () ->
                GGCommons.processArgs(COMPONENT, new String[]{"-m", "BOGUS_MODE"}, null));
    }

    @Test
    void customAppOptionsArePreservedAndAllFlagsParseTogether() {
        Options appOptions = new Options();
        appOptions.addOption("x", "extra", true, "an app-specific option");

        ParsedCommandLine pcl = GGCommons.processArgs(
                COMPONENT,
                new String[]{
                        "-t", "thing-7",
                        "-m", "STANDALONE", "./sa.json",
                        "-c", "ENV", "MY_CONFIG_VAR",
                        "-x", "appvalue"
                },
                appOptions);

        assertEquals("thing-7", pcl.thingName);
        assertEquals(ParsedCommandLine.Mode.STANDALONE, pcl.mode);
        assertEquals("./sa.json", pcl.standaloneConfigPath);
        assertArrayEquals(new String[]{"ENV", "MY_CONFIG_VAR"}, pcl.configArgs);
        // The custom app option is parsed into the same CommandLine.
        assertEquals("appvalue", pcl.commandLine.getOptionValue("x"));
    }
}
