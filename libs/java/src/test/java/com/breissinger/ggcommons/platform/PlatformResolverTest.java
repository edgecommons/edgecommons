/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.platform;

import org.junit.jupiter.api.Test;

import java.util.Map;
import java.util.function.Predicate;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for the pure {@link PlatformResolver} — the precedence resolver (DESIGN-core §4), the
 * auto-detector (§5), the IPC-lock validation (§4.1), and identity resolution (§6.2). Exercised in
 * isolation with injected environments and a filesystem probe so the suite is the Phase-0 oracle.
 */
class PlatformResolverTest {

    private static final Predicate<String> NO_FILES = p -> false;
    private static final Predicate<String> ALL_FILES = p -> true;

    // ---------- detectPlatform ----------

    @Test
    void detectGreengrassFromIpcSocketEnv() {
        Map<String, String> env = Map.of(PlatformResolver.ENV_GG_IPC_SOCKET, "/run/gg.sock");
        assertEquals(Platform.GREENGRASS, PlatformResolver.detectPlatform(env, NO_FILES));
    }

    @Test
    void detectGreengrassFromSvcuidEnv() {
        Map<String, String> env = Map.of(PlatformResolver.ENV_GG_SVCUID, "abc123");
        assertEquals(Platform.GREENGRASS, PlatformResolver.detectPlatform(env, NO_FILES));
    }

    @Test
    void detectKubernetesFromTokenFile() {
        Predicate<String> onlyToken = PlatformResolver.K8S_SA_TOKEN_PATH::equals;
        assertEquals(Platform.KUBERNETES, PlatformResolver.detectPlatform(Map.of(), onlyToken));
    }

    @Test
    void detectKubernetesFromServiceHostEnv() {
        Map<String, String> env = Map.of(PlatformResolver.ENV_K8S_SERVICE_HOST, "10.0.0.1");
        assertEquals(Platform.KUBERNETES, PlatformResolver.detectPlatform(env, NO_FILES));
    }

    @Test
    void detectHostWhenNoSignals() {
        assertEquals(Platform.HOST, PlatformResolver.detectPlatform(Map.of(), NO_FILES));
    }

    @Test
    void greengrassWinsOverKubernetesWhenBothSignalsPresent() {
        // A containerized Nucleus component can set both; GREENGRASS must win (load-bearing order).
        Map<String, String> env = Map.of(
                PlatformResolver.ENV_GG_SVCUID, "uid",
                PlatformResolver.ENV_K8S_SERVICE_HOST, "10.0.0.1");
        assertEquals(Platform.GREENGRASS, PlatformResolver.detectPlatform(env, ALL_FILES));
    }

    @Test
    void emptyEnvValueIsNotASignal() {
        Map<String, String> env = Map.of(PlatformResolver.ENV_GG_SVCUID, "");
        assertEquals(Platform.HOST, PlatformResolver.detectPlatform(env, NO_FILES));
    }

    @Test
    void publicDetectUsesRealFilesystemProbe() {
        // The token path does not exist on the test host -> HOST, with no env signals.
        assertEquals(Platform.HOST, PlatformResolver.detectPlatform(Map.of()));
    }

    // ---------- resolveProfile: profile defaults ----------

    @Test
    void resolveGreengrassExplicitGivesIpcAndGgConfig() {
        var inputs = new PlatformResolver.ResolverInputs(Platform.GREENGRASS, null, null, null);
        ResolvedProfile r = PlatformResolver.resolveProfile(inputs, Map.of());

        assertEquals(Platform.GREENGRASS, r.platform());
        assertEquals(Transport.IPC, r.transport());
        assertArrayEquals(new String[]{"GG_CONFIG"}, r.configSource());
        assertEquals(PlatformResolver.DEFAULT_IDENTITY, r.identity());
    }

    @Test
    void resolveHostExplicitGivesMqttAndGgConfigInPhase0() {
        // Phase 0 deliberately keeps HOST's default config source at GG_CONFIG (not FILE).
        var inputs = new PlatformResolver.ResolverInputs(Platform.HOST, null, null, null);
        ResolvedProfile r = PlatformResolver.resolveProfile(inputs, Map.of());

        assertEquals(Platform.HOST, r.platform());
        assertEquals(Transport.MQTT, r.transport());
        assertArrayEquals(new String[]{"GG_CONFIG"}, r.configSource());
    }

    @Test
    void resolveAutoWithNoSignalsDetectsHost() {
        var inputs = new PlatformResolver.ResolverInputs(null, null, null, null);
        ResolvedProfile r = PlatformResolver.resolveProfile(inputs, Map.of());
        assertEquals(Platform.HOST, r.platform());
        assertEquals(Transport.MQTT, r.transport());
    }

    @Test
    void resolveAutoWithGreengrassEnvDetectsGreengrass() {
        var inputs = new PlatformResolver.ResolverInputs(null, null, null, null);
        ResolvedProfile r = PlatformResolver.resolveProfile(
                inputs, Map.of(PlatformResolver.ENV_GG_IPC_SOCKET, "/run/gg.sock"));
        assertEquals(Platform.GREENGRASS, r.platform());
        assertEquals(Transport.IPC, r.transport());
    }

    // ---------- resolveProfile: explicit overrides ----------

    @Test
    void explicitConfigArgsOverrideProfileDefault() {
        var inputs = new PlatformResolver.ResolverInputs(
                Platform.GREENGRASS, null, new String[]{"FILE", "/etc/cfg.json"}, null);
        ResolvedProfile r = PlatformResolver.resolveProfile(inputs, Map.of());
        assertArrayEquals(new String[]{"FILE", "/etc/cfg.json"}, r.configSource());
    }

    @Test
    void explicitTransportOverridesProfileDefault() {
        // HOST normally derives MQTT; an explicit MQTT is still MQTT (and legal).
        var inputs = new PlatformResolver.ResolverInputs(Platform.HOST, Transport.MQTT, null, null);
        ResolvedProfile r = PlatformResolver.resolveProfile(inputs, Map.of());
        assertEquals(Transport.MQTT, r.transport());
    }

    @Test
    void explicitThingOverridesEnvProbe() {
        var inputs = new PlatformResolver.ResolverInputs(Platform.HOST, null, null, "my-thing");
        ResolvedProfile r = PlatformResolver.resolveProfile(
                inputs, Map.of(PlatformResolver.ENV_THING_NAME, "env-thing"));
        assertEquals("my-thing", r.identity());
    }

    // ---------- resolveProfile: failures ----------

    @Test
    void resolveKubernetesExplicitGivesMqttAndConfigMap() {
        // Phase 1a: KUBERNETES now resolves cleanly (no fail-fast) to MQTT + the CONFIGMAP source.
        var inputs = new PlatformResolver.ResolverInputs(Platform.KUBERNETES, null, null, null);
        ResolvedProfile r = PlatformResolver.resolveProfile(inputs, Map.of());

        assertEquals(Platform.KUBERNETES, r.platform());
        assertEquals(Transport.MQTT, r.transport());
        assertArrayEquals(new String[]{"CONFIGMAP"}, r.configSource());
    }

    @Test
    void resolveAutoWithServiceAccountTokenDetectsKubernetes() {
        // A SA-token pod auto-detects to KUBERNETES and gets MQTT + CONFIGMAP.
        var inputs = new PlatformResolver.ResolverInputs(null, null, null, null);
        ResolvedProfile r = PlatformResolver.resolveProfile(
                inputs, Map.of(PlatformResolver.ENV_K8S_SERVICE_HOST, "10.0.0.1"));
        assertEquals(Platform.KUBERNETES, r.platform());
        assertEquals(Transport.MQTT, r.transport());
        assertArrayEquals(new String[]{"CONFIGMAP"}, r.configSource());
    }

    @Test
    void resolveIpcOnKubernetesFailsTheIpcLock() {
        // The IPC lock still holds on KUBERNETES (only the Nucleus provides the IPC socket).
        var inputs = new PlatformResolver.ResolverInputs(Platform.KUBERNETES, Transport.IPC, null, null);
        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class,
                () -> PlatformResolver.resolveProfile(inputs, Map.of()));
        assertTrue(ex.getMessage().contains("IPC transport requires --platform GREENGRASS"));
    }

    @Test
    void resolveIpcOnHostFailsTheIpcLock() {
        var inputs = new PlatformResolver.ResolverInputs(Platform.HOST, Transport.IPC, null, null);
        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class,
                () -> PlatformResolver.resolveProfile(inputs, Map.of()));
        assertTrue(ex.getMessage().contains("IPC transport requires --platform GREENGRASS"));
    }

    // ---------- validate ----------

    @Test
    void validateRejectsIpcOnNonGreengrass() {
        assertThrows(IllegalArgumentException.class,
                () -> PlatformResolver.validate(Platform.HOST, Transport.IPC));
        assertThrows(IllegalArgumentException.class,
                () -> PlatformResolver.validate(Platform.KUBERNETES, Transport.IPC));
    }

    @Test
    void validateAcceptsLegalCombos() {
        assertDoesNotThrow(() -> PlatformResolver.validate(Platform.GREENGRASS, Transport.IPC));
        assertDoesNotThrow(() -> PlatformResolver.validate(Platform.HOST, Transport.MQTT));
        assertDoesNotThrow(() -> PlatformResolver.validate(Platform.KUBERNETES, Transport.MQTT));
    }

    // ---------- resolveIdentity ----------

    @Test
    void resolveIdentityPrefersExplicitThing() {
        assertEquals("t1", PlatformResolver.resolveIdentity("t1", Platform.GREENGRASS, Map.of()));
    }

    @Test
    void resolveIdentityFallsBackToEnv() {
        assertEquals("env-thing", PlatformResolver.resolveIdentity(
                null, Platform.HOST, Map.of(PlatformResolver.ENV_THING_NAME, "env-thing")));
    }

    @Test
    void resolveIdentityDefaultsWhenNothingAvailable() {
        assertEquals(PlatformResolver.DEFAULT_IDENTITY,
                PlatformResolver.resolveIdentity(null, Platform.HOST, Map.of()));
    }

    @Test
    void resolveIdentityTreatsEmptyEnvAsAbsent() {
        // Cross-language parity (DESIGN-core §12 #2): a present-but-empty AWS_IOT_THING_NAME
        // is treated as absent and falls through to the default, matching Python/Rust/TS.
        assertEquals(PlatformResolver.DEFAULT_IDENTITY, PlatformResolver.resolveIdentity(
                null, Platform.HOST, Map.of(PlatformResolver.ENV_THING_NAME, "")));
    }

    @Test
    void resolveIdentityHandlesNullEnv() {
        assertEquals(PlatformResolver.DEFAULT_IDENTITY,
                PlatformResolver.resolveIdentity(null, Platform.HOST, null));
    }

    // ---------- profiles + enums ----------

    @Test
    void profilesContainAllThreePlatforms() {
        assertEquals(3, PlatformResolver.PROFILES.size());
        assertTrue(PlatformResolver.PROFILES.containsKey(Platform.GREENGRASS));
        assertTrue(PlatformResolver.PROFILES.containsKey(Platform.HOST));
        assertTrue(PlatformResolver.PROFILES.containsKey(Platform.KUBERNETES));
    }

    @Test
    void kubernetesProfileExposesMqttAndConfigMap() {
        PlatformProfile p = PlatformResolver.PROFILES.get(Platform.KUBERNETES);
        assertEquals(Transport.MQTT, p.transport());
        assertEquals("CONFIGMAP", p.configSource());
    }

    @Test
    void enumsDeclareExpectedValues() {
        assertEquals(3, Platform.values().length);
        assertEquals(Platform.KUBERNETES, Platform.valueOf("KUBERNETES"));
        assertEquals(2, Transport.values().length);
        assertEquals(Transport.IPC, Transport.valueOf("IPC"));
    }

    @Test
    void profileRecordExposesItsFields() {
        PlatformProfile p = PlatformResolver.PROFILES.get(Platform.GREENGRASS);
        assertEquals(Transport.IPC, p.transport());
        assertEquals("GG_CONFIG", p.configSource());
    }
}
