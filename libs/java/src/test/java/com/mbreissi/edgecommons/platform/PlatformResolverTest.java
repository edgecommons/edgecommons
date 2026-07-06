/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.platform;

import com.mbreissi.edgecommons.config.ConfigManager;
import org.junit.jupiter.api.Test;

import java.nio.file.Path;
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
    void resolveHostExplicitGivesMqttAndFile() {
        // Phase 1 (§12 #1): HOST defaults its config source to FILE (GG_CONFIG needs Nucleus IPC).
        var inputs = new PlatformResolver.ResolverInputs(Platform.HOST, null, null, null);
        ResolvedProfile r = PlatformResolver.resolveProfile(inputs, Map.of());

        assertEquals(Platform.HOST, r.platform());
        assertEquals(Transport.MQTT, r.transport());
        assertArrayEquals(new String[]{"FILE"}, r.configSource());
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

    // ---------- resolveIdentity: KUBERNETES Downward-API tier (FR-RT-7) ----------

    @Test
    void k8sIdentityFromEdgeCommonsThingNameEnv() {
        // EDGECOMMONS_THING_NAME (the chart-mapped annotation) is the top of the KUBERNETES tier.
        assertEquals("annotated-thing", PlatformResolver.resolveIdentity(
                null, Platform.KUBERNETES, Map.of(PlatformResolver.ENV_K8S_THING_NAME, "annotated-thing")));
    }

    @Test
    void k8sIdentityFromPodNameWhenNoAnnotation() {
        // With no EDGECOMMONS_THING_NAME, the Downward-API POD_NAME (metadata.name) is used.
        assertEquals("my-pod-abc123", PlatformResolver.resolveIdentity(
                null, Platform.KUBERNETES, Map.of(PlatformResolver.ENV_K8S_POD_NAME, "my-pod-abc123")));
    }

    @Test
    void k8sAnnotationPrecedesPodName() {
        assertEquals("annotated", PlatformResolver.resolveIdentity(null, Platform.KUBERNETES, Map.of(
                PlatformResolver.ENV_K8S_THING_NAME, "annotated",
                PlatformResolver.ENV_K8S_POD_NAME, "pod-xyz")));
    }

    @Test
    void k8sTierPrecedesAwsIotThingNameOnKubernetes() {
        // On KUBERNETES the Downward-API tier wins over the generic AWS_IOT_THING_NAME probe.
        assertEquals("pod-1", PlatformResolver.resolveIdentity(null, Platform.KUBERNETES, Map.of(
                PlatformResolver.ENV_K8S_POD_NAME, "pod-1",
                PlatformResolver.ENV_THING_NAME, "iot-thing")));
    }

    @Test
    void k8sTierIsIgnoredOnNonKubernetesPlatforms() {
        // The k8s env vars must NOT affect identity on HOST/GREENGRASS — the generic probe wins.
        Map<String, String> env = Map.of(
                PlatformResolver.ENV_K8S_THING_NAME, "annotated",
                PlatformResolver.ENV_K8S_POD_NAME, "pod-1",
                PlatformResolver.ENV_THING_NAME, "iot-thing");
        assertEquals("iot-thing", PlatformResolver.resolveIdentity(null, Platform.HOST, env));
        assertEquals("iot-thing", PlatformResolver.resolveIdentity(null, Platform.GREENGRASS, env));
    }

    @Test
    void k8sFallsThroughToGenericProbeWhenTierAbsent() {
        // No EDGECOMMONS_THING_NAME / POD_NAME on k8s -> the generic AWS_IOT_THING_NAME probe applies.
        assertEquals("iot-thing", PlatformResolver.resolveIdentity(
                null, Platform.KUBERNETES, Map.of(PlatformResolver.ENV_THING_NAME, "iot-thing")));
    }

    @Test
    void k8sTreatsEmptyTierValuesAsAbsent() {
        // Empty EDGECOMMONS_THING_NAME and POD_NAME are ignored; falls through to the default.
        Map<String, String> env = Map.of(
                PlatformResolver.ENV_K8S_THING_NAME, "",
                PlatformResolver.ENV_K8S_POD_NAME, "");
        assertEquals(PlatformResolver.DEFAULT_IDENTITY,
                PlatformResolver.resolveIdentity(null, Platform.KUBERNETES, env));
    }

    @Test
    void explicitThingOverridesK8sTier() {
        // -t/--thing is the highest precedence even on KUBERNETES.
        assertEquals("explicit", PlatformResolver.resolveIdentity("explicit", Platform.KUBERNETES, Map.of(
                PlatformResolver.ENV_K8S_THING_NAME, "annotated",
                PlatformResolver.ENV_K8S_POD_NAME, "pod-1")));
    }

    @Test
    void resolvedK8sIdentityIsRawAndSanitizedWhenInterpolated() {
        // FR-RT-7: the resolver returns the raw value (no mangling), but a hostile pod name MUST still
        // pass the existing template-variable sanitization wherever it is interpolated into a path/topic
        // (no path traversal, no MQTT wildcards). Mirrors the Rust/TS parity test.
        String identity = PlatformResolver.resolveIdentity(
                null, Platform.KUBERNETES, Map.of(PlatformResolver.ENV_K8S_POD_NAME, "../../etc/passwd"));
        assertEquals("../../etc/passwd", identity, "resolver returns the raw value");

        // Downstream: the identity is sanitized by ConfigManager.resolveTemplate when interpolated.
        ConfigManager cm = new ConfigManager() {
            @Override
            public String getThingName() {
                return identity;
            }
        };
        assertEquals("/logs/____etc_passwd.log", cm.resolveTemplate("/logs/{ThingName}.log"));
    }

    // ---------- resolveMessagingConfigPath (FR-MSG-1) ----------

    @Test
    void messagingPathExplicitAlwaysWins() {
        // An explicit --transport MQTT <path> is honored unchanged, even under CONFIGMAP+MQTT.
        assertEquals("/custom/msg.json", PlatformResolver.resolveMessagingConfigPath(
                "/custom/msg.json", Transport.MQTT, new String[]{"CONFIGMAP"}));
    }

    @Test
    void messagingPathDefaultsToConfigMapFileUnderConfigMapMqtt() {
        // No explicit path + MQTT + CONFIGMAP -> the resolved ConfigMap file (default dir/key).
        assertEquals(Path.of(PlatformResolver.CONFIGMAP_DEFAULT_MOUNT_DIR)
                        .resolve(PlatformResolver.CONFIGMAP_DEFAULT_KEY).toString(),
                PlatformResolver.resolveMessagingConfigPath(null, Transport.MQTT, new String[]{"CONFIGMAP"}));
    }

    @Test
    void messagingPathUsesCustomConfigMapDirAndKey() {
        assertEquals(Path.of("/mnt/cfg").resolve("app.json").toString(),
                PlatformResolver.resolveMessagingConfigPath(
                        null, Transport.MQTT, new String[]{"CONFIGMAP", "/mnt/cfg", "app.json"}));
    }

    @Test
    void messagingPathUsesCustomConfigMapDirWithDefaultKey() {
        assertEquals(Path.of("/mnt/cfg").resolve(PlatformResolver.CONFIGMAP_DEFAULT_KEY).toString(),
                PlatformResolver.resolveMessagingConfigPath(
                        null, Transport.MQTT, new String[]{"CONFIGMAP", "/mnt/cfg"}));
    }

    @Test
    void messagingPathConfigMapTokenIsCaseInsensitive() {
        assertEquals(Path.of(PlatformResolver.CONFIGMAP_DEFAULT_MOUNT_DIR)
                        .resolve(PlatformResolver.CONFIGMAP_DEFAULT_KEY).toString(),
                PlatformResolver.resolveMessagingConfigPath(null, Transport.MQTT, new String[]{"configmap"}));
    }

    @Test
    void messagingPathNullForMqttWithNonConfigMapSource() {
        // Only CONFIGMAP synthesizes a default messaging path; FILE/GG_CONFIG do not, so HOST (FILE)
        // and explicit non-CONFIGMAP sources still require an explicit MQTT path.
        assertNull(PlatformResolver.resolveMessagingConfigPath(null, Transport.MQTT, new String[]{"GG_CONFIG"}));
        assertNull(PlatformResolver.resolveMessagingConfigPath(null, Transport.MQTT, new String[]{"FILE", "c.json"}));
    }

    @Test
    void messagingPathNullForNonMqttTransport() {
        // IPC never carries a messaging-config path, even with a CONFIGMAP source.
        assertNull(PlatformResolver.resolveMessagingConfigPath(null, Transport.IPC, new String[]{"CONFIGMAP"}));
    }

    @Test
    void messagingPathNullForEmptyConfigSource() {
        assertNull(PlatformResolver.resolveMessagingConfigPath(null, Transport.MQTT, new String[]{}));
        assertNull(PlatformResolver.resolveMessagingConfigPath(null, Transport.MQTT, null));
    }

    // ---------- resolveProfile: messaging-config path end-to-end (FR-MSG-1) ----------

    @Test
    void resolveKubernetesDefaultsMessagingPathToConfigMapFile() {
        var inputs = new PlatformResolver.ResolverInputs(Platform.KUBERNETES, null, null, null);
        ResolvedProfile r = PlatformResolver.resolveProfile(inputs, Map.of());
        assertEquals(Path.of(PlatformResolver.CONFIGMAP_DEFAULT_MOUNT_DIR)
                .resolve(PlatformResolver.CONFIGMAP_DEFAULT_KEY).toString(), r.messagingConfigPath());
    }

    @Test
    void resolveKubernetesHonorsExplicitMessagingPath() {
        var inputs = new PlatformResolver.ResolverInputs(
                Platform.KUBERNETES, Transport.MQTT, null, null, "/explicit/msg.json");
        ResolvedProfile r = PlatformResolver.resolveProfile(inputs, Map.of());
        assertEquals("/explicit/msg.json", r.messagingConfigPath());
    }

    @Test
    void resolveHostLeavesMessagingPathNullWhenAbsent() {
        // HOST+MQTT with no explicit path -> null (HOST still requires an explicit path; FR-MSG-1).
        var inputs = new PlatformResolver.ResolverInputs(Platform.HOST, null, null, null);
        ResolvedProfile r = PlatformResolver.resolveProfile(inputs, Map.of());
        assertNull(r.messagingConfigPath());
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

    // ---------- profileHealthEnabled (FR-HB-1 / FR-RT-3) ----------

    @Test
    void profileHealthEnabledTrueOnlyForKubernetes() {
        assertTrue(PlatformResolver.profileHealthEnabled(Platform.KUBERNETES),
                "health server defaults on for KUBERNETES");
        assertFalse(PlatformResolver.profileHealthEnabled(Platform.HOST));
        assertFalse(PlatformResolver.profileHealthEnabled(Platform.GREENGRASS));
    }

    @Test
    void profileHealthEnabledFalseForNullPlatform() {
        assertFalse(PlatformResolver.profileHealthEnabled(null));
    }

    @Test
    void profileHealthEnabledFieldMatchesProfileTable() {
        assertTrue(PlatformResolver.PROFILES.get(Platform.KUBERNETES).healthEnabled());
        assertFalse(PlatformResolver.PROFILES.get(Platform.HOST).healthEnabled());
        assertFalse(PlatformResolver.PROFILES.get(Platform.GREENGRASS).healthEnabled());
    }

    // ---------- profileMetricTarget (FR-MET-1 / FR-RT-3) ----------

    @Test
    void profileMetricTargetIsPrometheusOnlyForKubernetes() {
        assertEquals(PlatformResolver.METRIC_TARGET_PROMETHEUS,
                PlatformResolver.profileMetricTarget(Platform.KUBERNETES),
                "metric target defaults to prometheus for KUBERNETES");
        assertNull(PlatformResolver.profileMetricTarget(Platform.HOST));
        assertNull(PlatformResolver.profileMetricTarget(Platform.GREENGRASS));
    }

    @Test
    void profileMetricTargetNullForNullPlatform() {
        assertNull(PlatformResolver.profileMetricTarget(null));
    }

    @Test
    void profileMetricTargetFieldMatchesProfileTable() {
        assertEquals("prometheus", PlatformResolver.PROFILES.get(Platform.KUBERNETES).metricTarget());
        assertNull(PlatformResolver.PROFILES.get(Platform.HOST).metricTarget());
        assertNull(PlatformResolver.PROFILES.get(Platform.GREENGRASS).metricTarget());
    }

    // ---------- profileCredentialsKeyProvider (FR-CRED-3 / FR-CRED-6 / FR-RT-3) ----------

    @Test
    void profileCredentialsKeyProviderIsEnvOnlyForKubernetes() {
        assertEquals(PlatformResolver.CREDENTIALS_KEY_PROVIDER_ENV,
                PlatformResolver.profileCredentialsKeyProvider(Platform.KUBERNETES),
                "credentials key provider defaults to env for KUBERNETES");
        assertNull(PlatformResolver.profileCredentialsKeyProvider(Platform.HOST));
        assertNull(PlatformResolver.profileCredentialsKeyProvider(Platform.GREENGRASS));
    }

    @Test
    void profileCredentialsKeyProviderNullForNullPlatform() {
        assertNull(PlatformResolver.profileCredentialsKeyProvider(null));
    }

    @Test
    void profileCredentialsKeyProviderFieldMatchesProfileTable() {
        assertEquals("env", PlatformResolver.PROFILES.get(Platform.KUBERNETES).credentialsKeyProvider());
        assertNull(PlatformResolver.PROFILES.get(Platform.HOST).credentialsKeyProvider());
        assertNull(PlatformResolver.PROFILES.get(Platform.GREENGRASS).credentialsKeyProvider());
    }
}
