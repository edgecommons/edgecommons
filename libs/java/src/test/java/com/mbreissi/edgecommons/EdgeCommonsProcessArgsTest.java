/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons;

import com.mbreissi.edgecommons.platform.Platform;
import com.mbreissi.edgecommons.platform.PlatformResolver;
import com.mbreissi.edgecommons.platform.Transport;
import org.apache.commons.cli.Options;
import org.junit.jupiter.api.Test;

import java.nio.file.Path;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for the static {@link EdgeCommons#processArgs(String, String[], Options)} argument
 * parser, re-pointed from the removed {@code -m/--mode} flag to the two-axis
 * {@code --platform}/{@code --transport} contract (DESIGN-core §6.1). Covers profile defaults, the
 * explicit flags, the MQTT messaging-config path, the IPC-lock guard and the legacy-flag rejection.
 * The {@code --help}/{@code -h} branch is deliberately NOT exercised because it calls
 * {@code System.exit(0)}.
 */
class EdgeCommonsProcessArgsTest {

    private static final String COMPONENT = "com.example.TestComponent";

    @Test
    void defaultConfigSourceComesFromPlatformProfile() {
        // -c absent -> default comes from the resolved platform profile: GG_CONFIG on GREENGRASS,
        // FILE on HOST (Phase 1, §12 #1).
        ParsedCommandLine host = EdgeCommons.processArgs(
                COMPONENT, new String[]{"--platform", "HOST", "--transport", "MQTT", "./msg.json"}, null);
        assertArrayEquals(new String[]{"FILE"}, host.configArgs);
        assertNotNull(host.commandLine);

        ParsedCommandLine gg = EdgeCommons.processArgs(
                COMPONENT, new String[]{"--platform", "GREENGRASS"}, null);
        assertArrayEquals(new String[]{"GG_CONFIG"}, gg.configArgs);
    }

    @Test
    void greengrassPlatformDerivesIpcTransport() {
        ParsedCommandLine pcl = EdgeCommons.processArgs(
                COMPONENT, new String[]{"--platform", "GREENGRASS"}, null);

        assertEquals(Platform.GREENGRASS, pcl.platform);
        assertEquals(Transport.IPC, pcl.transport);
        assertNull(pcl.standaloneConfigPath);
    }

    @Test
    void hostPlatformWithMqttTransportTakesMessagingConfigPath() {
        ParsedCommandLine pcl = EdgeCommons.processArgs(
                COMPONENT,
                new String[]{"--platform", "HOST", "--transport", "MQTT", "./standalone-messaging.json"},
                null);

        assertEquals(Platform.HOST, pcl.platform);
        assertEquals(Transport.MQTT, pcl.transport);
        assertEquals("./standalone-messaging.json", pcl.standaloneConfigPath);
    }

    @Test
    void transportTokenIsCaseInsensitive() {
        ParsedCommandLine pcl = EdgeCommons.processArgs(
                COMPONENT,
                new String[]{"--platform", "host", "--transport", "mqtt", "./msg.json"},
                null);

        assertEquals(Platform.HOST, pcl.platform);
        assertEquals(Transport.MQTT, pcl.transport);
        assertEquals("./msg.json", pcl.standaloneConfigPath);
    }

    @Test
    void explicitThingTakesFullStringValue() {
        // Guards against the historical bug that truncated the thing name to one char.
        ParsedCommandLine pcl = EdgeCommons.processArgs(
                COMPONENT, new String[]{"--platform", "GREENGRASS", "-t", "my-full-thing-name"}, null);

        assertEquals("my-full-thing-name", pcl.thingName);
    }

    @Test
    void longThingOptionTakesFullStringValue() {
        ParsedCommandLine pcl = EdgeCommons.processArgs(
                COMPONENT, new String[]{"--platform", "GREENGRASS", "--thing", "another-full-thing"}, null);

        assertEquals("another-full-thing", pcl.thingName);
    }

    @Test
    void explicitConfigSourceWithArgs() {
        ParsedCommandLine pcl = EdgeCommons.processArgs(
                COMPONENT, new String[]{"--platform", "GREENGRASS", "-c", "FILE", "./config.json"}, null);

        assertArrayEquals(new String[]{"FILE", "./config.json"}, pcl.configArgs);
        assertEquals(Transport.IPC, pcl.transport);
    }

    @Test
    void mqttTransportWithoutPathParsesButLeavesPathNull() {
        // Parsing must not require the messaging-config path; the requirement is enforced later when
        // the MQTT provider is actually built (MessagingClient), so mock-messaging collaborators can
        // still resolve args.
        ParsedCommandLine pcl = EdgeCommons.processArgs(
                COMPONENT, new String[]{"--platform", "HOST", "--transport", "MQTT"}, null);
        assertEquals(Transport.MQTT, pcl.transport);
        assertNull(pcl.standaloneConfigPath);
    }

    @Test
    void ipcOnHostFailsTheIpcLock() {
        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class, () ->
                EdgeCommons.processArgs(COMPONENT, new String[]{"--platform", "HOST", "--transport", "IPC"}, null));
        assertTrue(ex.getMessage().contains("IPC transport requires --platform GREENGRASS"));
    }

    @Test
    void unknownPlatformThrows() {
        assertThrows(IllegalArgumentException.class, () ->
                EdgeCommons.processArgs(COMPONENT, new String[]{"--platform", "BOGUS"}, null));
    }

    @Test
    void unknownTransportThrows() {
        assertThrows(IllegalArgumentException.class, () ->
                EdgeCommons.processArgs(COMPONENT, new String[]{"--platform", "HOST", "--transport", "BOGUS"}, null));
    }

    @Test
    void legacyShortModeFlagIsRejectedWithGuidance() {
        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class, () ->
                EdgeCommons.processArgs(COMPONENT, new String[]{"-m", "STANDALONE", "./msg.json"}, null));
        assertTrue(ex.getMessage().contains("--platform"));
        assertTrue(ex.getMessage().contains("--transport"));
    }

    @Test
    void legacyLongModeFlagIsRejectedWithGuidance() {
        assertThrows(IllegalArgumentException.class, () ->
                EdgeCommons.processArgs(COMPONENT, new String[]{"--mode", "GREENGRASS"}, null));
    }

    @Test
    void legacyAttachedModeFlagsAreRejectedNotNpe() {
        // Attached forms (--mode=X, -mX) must hit the legacy-flag guard with guidance,
        // not slip past into a half-parsed state / NPE (DESIGN-core §12 #3).
        for (String bad : new String[]{"--mode=GREENGRASS", "-mSTANDALONE"}) {
            IllegalArgumentException ex = assertThrows(IllegalArgumentException.class, () ->
                    EdgeCommons.processArgs(COMPONENT, new String[]{bad}, null),
                    "expected rejection for " + bad);
            assertTrue(ex.getMessage().contains("--platform"));
        }
    }

    @Test
    void kubernetesPlatformResolvesToMqttAndConfigMap() {
        // Phase 1a: --platform KUBERNETES resolves cleanly (no fail-fast) to MQTT + the CONFIGMAP
        // config source; no positional messaging-config path is required at parse time.
        ParsedCommandLine pcl = EdgeCommons.processArgs(
                COMPONENT, new String[]{"--platform", "KUBERNETES"}, null);
        assertEquals(Platform.KUBERNETES, pcl.platform);
        assertEquals(Transport.MQTT, pcl.transport);
        assertArrayEquals(new String[]{"CONFIGMAP"}, pcl.configArgs);
    }

    @Test
    void kubernetesDefaultsMessagingPathToConfigMapFile() {
        // FR-MSG-1: --platform KUBERNETES resolves to MQTT + CONFIGMAP, so the messaging-config path
        // defaults to the ConfigMap file (/etc/edgecommons/config.json) with no positional path. The
        // single mounted config.json then doubles as both the messaging config and the component config.
        ParsedCommandLine pcl = EdgeCommons.processArgs(
                COMPONENT, new String[]{"--platform", "KUBERNETES"}, null);
        assertEquals(Transport.MQTT, pcl.transport);
        assertArrayEquals(new String[]{"CONFIGMAP"}, pcl.configArgs);
        assertEquals(Path.of(PlatformResolver.CONFIGMAP_DEFAULT_MOUNT_DIR)
                .resolve(PlatformResolver.CONFIGMAP_DEFAULT_KEY).toString(), pcl.standaloneConfigPath);
    }

    @Test
    void kubernetesDefaultsMessagingPathFromCustomConfigMapArgs() {
        // The default uses the SAME mount dir/key the CONFIGMAP source resolves from -c CONFIGMAP.
        ParsedCommandLine pcl = EdgeCommons.processArgs(
                COMPONENT, new String[]{"--platform", "KUBERNETES", "-c", "CONFIGMAP", "/mnt/cfg", "app.json"}, null);
        assertArrayEquals(new String[]{"CONFIGMAP", "/mnt/cfg", "app.json"}, pcl.configArgs);
        assertEquals(Path.of("/mnt/cfg").resolve("app.json").toString(), pcl.standaloneConfigPath);
    }

    @Test
    void kubernetesHonorsExplicitMessagingPathOverConfigMapDefault() {
        // An explicit --transport MQTT <path> still wins under CONFIGMAP+MQTT (existing behavior).
        ParsedCommandLine pcl = EdgeCommons.processArgs(
                COMPONENT, new String[]{"--platform", "KUBERNETES", "--transport", "MQTT", "/custom/msg.json"}, null);
        assertEquals(Transport.MQTT, pcl.transport);
        assertEquals("/custom/msg.json", pcl.standaloneConfigPath);
    }

    @Test
    void hostMqttWithoutPathGetsNoConfigMapDefault() {
        // FR-MSG-1 only synthesizes a messaging path for CONFIGMAP: HOST defaults to FILE (not
        // CONFIGMAP), so HOST+MQTT with no explicit path leaves the messaging-config path null
        // (enforced later at provider build).
        ParsedCommandLine pcl = EdgeCommons.processArgs(
                COMPONENT, new String[]{"--platform", "HOST", "--transport", "MQTT"}, null);
        assertEquals(Transport.MQTT, pcl.transport);
        assertArrayEquals(new String[]{"FILE"}, pcl.configArgs);
        assertNull(pcl.standaloneConfigPath);
    }

    @Test
    void kubernetesPlatformRejectsIpcTransport() {
        // The IPC lock still holds on KUBERNETES (only the Nucleus provides the IPC socket).
        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class, () ->
                EdgeCommons.processArgs(COMPONENT, new String[]{"--platform", "KUBERNETES", "--transport", "IPC"}, null));
        assertTrue(ex.getMessage().contains("IPC transport requires --platform GREENGRASS"));
    }

    @Test
    void autoPlatformIsAcceptedAsAlias() {
        // 'auto' (explicit) behaves like omitting --platform: detection runs. On the test host with
        // no Nucleus signals this resolves to HOST, so a messaging-config path is required.
        ParsedCommandLine pcl = EdgeCommons.processArgs(
                COMPONENT, new String[]{"--platform", "auto", "--transport", "MQTT", "./msg.json"}, null);
        assertEquals(Transport.MQTT, pcl.transport);
        assertEquals("./msg.json", pcl.standaloneConfigPath);
    }

    @Test
    void customAppOptionsArePreservedAndAllFlagsParseTogether() {
        Options appOptions = new Options();
        appOptions.addOption("x", "extra", true, "an app-specific option");

        ParsedCommandLine pcl = EdgeCommons.processArgs(
                COMPONENT,
                new String[]{
                        "-t", "thing-7",
                        "--platform", "HOST",
                        "--transport", "MQTT", "./sa.json",
                        "-c", "ENV", "MY_CONFIG_VAR",
                        "-x", "appvalue"
                },
                appOptions);

        assertEquals("thing-7", pcl.thingName);
        assertEquals(Platform.HOST, pcl.platform);
        assertEquals(Transport.MQTT, pcl.transport);
        assertEquals("./sa.json", pcl.standaloneConfigPath);
        assertArrayEquals(new String[]{"ENV", "MY_CONFIG_VAR"}, pcl.configArgs);
        // The custom app option is parsed into the same CommandLine.
        assertEquals("appvalue", pcl.commandLine.getOptionValue("x"));
    }
}
