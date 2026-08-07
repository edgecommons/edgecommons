/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.proto.v1.EcValue;
import com.mbreissi.edgecommons.proto.v1.EdgeCommonsMessage;
import com.google.gson.Gson;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.google.gson.JsonPrimitive;
import com.google.protobuf.InvalidProtocolBufferException;
import org.junit.jupiter.api.Test;

import java.math.BigDecimal;
import java.time.Duration;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * The intake-boundary guarantee (D-NC1) against a document shaped exactly like the one a
 * Greengrass deployment delivers — <b>reproduced without a device</b>.
 *
 * <p>The Nucleus is a Java process and its configuration store round-trips JSON numbers as
 * {@code double}s, so a merged {@code pollIntervalMs: 5000} comes back over IPC as the Java
 * {@code Double} {@code 5000.0}. {@link com.mbreissi.edgecommons.config.provider.GreengrassConfigProvider}
 * turns that {@code Map<String,Object>} into a Gson tree with {@code gson.toJson(...)} followed by
 * {@code gson.fromJson(..., JsonObject.class)}. {@link #greengrassIpcPayload()} builds the same map
 * and {@link #asGreengrassProviderWould(Map)} performs the same conversion, so these tests exercise
 * the real delivered shape with no Nucleus, no IPC, and no lab hardware.
 *
 * <p>Assertions are on serialized text: Gson compares numeric {@link JsonElement}s by their
 * {@code double} value, so {@code 5000} and {@code 5000.0} are {@code equals} — see
 * {@link ConfigNumbersTest}.
 */
class ConfigManagerNumericCanonicalizationTest {

    // ------------------------------------------------------------------
    // The Greengrass-shaped fixture
    // ------------------------------------------------------------------

    /**
     * The {@code Map<String,Object>} the Greengrass IPC SDK hands back for this component: every
     * configured number is a Java {@link Double}, because the Nucleus config store keeps JSON
     * numbers as doubles. Strings and genuinely-fractional values are present too, so the test
     * proves the pass is selective rather than blanket.
     */
    private static Map<String, Object> greengrassIpcPayload() {
        Map<String, Object> logging = new LinkedHashMap<>();
        logging.put("level", "INFO");

        Map<String, Object> heartbeat = new LinkedHashMap<>();
        heartbeat.put("intervalSecs", 30.0);

        Map<String, Object> metricTargetConfig = new LinkedHashMap<>();
        metricTargetConfig.put("port", 9090.0);
        Map<String, Object> metricEmission = new LinkedHashMap<>();
        metricEmission.put("target", "prometheus");
        metricEmission.put("targetConfig", metricTargetConfig);

        // The component-owned subtree: the one core never covered, and the one that killed the
        // Rust adapter on the device.
        Map<String, Object> global = new LinkedHashMap<>();
        global.put("pollIntervalMs", 5000.0);
        global.put("timeoutMultiplier", 3.0);
        global.put("deadbandPercent", 1.5);           // genuinely fractional - must survive

        Map<String, Object> instance = new LinkedHashMap<>();
        instance.put("id", "plc-1");
        instance.put("slot", 0.0);
        instance.put("rpiMs", 100.0);
        instance.put("backoff", List.of(50.0, 250.0, 1000.0));
        instance.put("scale", 0.125);                  // genuinely fractional - must survive

        Map<String, Object> component = new LinkedHashMap<>();
        component.put("global", global);
        component.put("instances", List.of(instance));

        // Envelope tags are in scope (D-NC5): a tag value must not land on the wire as a different
        // EcValue type depending on which store delivered the same logical config.
        Map<String, Object> tags = new LinkedHashMap<>();
        tags.put("line", 3.0);
        tags.put("site", "dallas");

        Map<String, Object> componentConfig = new LinkedHashMap<>();
        componentConfig.put("logging", logging);
        componentConfig.put("heartbeat", heartbeat);
        componentConfig.put("metricEmission", metricEmission);
        componentConfig.put("tags", tags);
        componentConfig.put("component", component);
        return componentConfig;
    }

    /** The exact Gson conversion {@code GreengrassConfigProvider.loadConfiguration()} performs. */
    private static JsonObject asGreengrassProviderWould(Map<String, Object> ipcValue) {
        Gson gson = new Gson();
        return gson.fromJson(gson.toJson(ipcValue), JsonObject.class);
    }

    private static JsonObject greengrassShapedConfig() {
        return asGreengrassProviderWould(greengrassIpcPayload());
    }

    private static ConfigManager manager(JsonObject config) {
        return new ConfigManager("com.test.TestComponent", "TestComponent", "gw-01", null, config);
    }

    /**
     * A strict typed consumer — what a Rust component's {@code serde} derive does with a
     * {@code u64} field, and what actually killed the adapter at startup on {@code lab-5950x}.
     */
    private static long strictU64(JsonElement element) {
        JsonPrimitive primitive = element.getAsJsonPrimitive();
        BigDecimal value = primitive.getAsBigDecimal();
        if (value.scale() > 0) {
            throw new IllegalArgumentException(
                    "invalid type: floating point `" + primitive.getAsString() + "`, expected u64");
        }
        return value.longValueExact();
    }

    // ------------------------------------------------------------------
    // Fixture fidelity + the device-free defect reproduction
    // ------------------------------------------------------------------

    @Test
    void theGreengrassShapedFixtureReallyCarriesDoubles() {
        JsonObject raw = greengrassShapedConfig();

        assertEquals("30.0", raw.getAsJsonObject("heartbeat").get("intervalSecs").toString());
        assertEquals("9090.0", raw.getAsJsonObject("metricEmission")
                .getAsJsonObject("targetConfig").get("port").toString());
        assertEquals("5000.0", raw.getAsJsonObject("component")
                .getAsJsonObject("global").get("pollIntervalMs").toString());
        assertEquals("100.0", raw.getAsJsonObject("component")
                .getAsJsonArray("instances").get(0).getAsJsonObject().get("rpiMs").toString());
        assertEquals("3.0", raw.getAsJsonObject("tags").get("line").toString());
    }

    @Test
    void aStrictConsumerFailsOnTheRawDocumentAndParsesTheDeliveredOne() {
        JsonObject raw = greengrassShapedConfig();

        // The observed on-device failure, reproduced with no Nucleus and no device.
        assertEquals("invalid type: floating point `5000.0`, expected u64",
                assertThrows(IllegalArgumentException.class,
                        () -> strictU64(raw.getAsJsonObject("component")
                                .getAsJsonObject("global").get("pollIntervalMs")))
                        .getMessage());

        // The same document, once it has passed the configuration intake boundary.
        ConfigManager cm = manager(raw);
        assertEquals(5000L, strictU64(cm.getGlobalConfig().get("pollIntervalMs")));
        assertEquals(100L, strictU64(cm.getInstanceConfig("plc-1").get("rpiMs")));
    }

    // ------------------------------------------------------------------
    // The committed snapshot is canonical everywhere
    // ------------------------------------------------------------------

    @Test
    void startupSnapshotIsCanonicalAcrossLibraryComponentAndTagSections() {
        ConfigManager cm = manager(greengrassShapedConfig());

        // Library-owned sections.
        JsonObject full = cm.getFullConfig();
        assertEquals("30", full.getAsJsonObject("heartbeat").get("intervalSecs").toString());
        assertEquals("9090", full.getAsJsonObject("metricEmission")
                .getAsJsonObject("targetConfig").get("port").toString());

        // component.global - the subtree core never covered.
        JsonObject global = cm.getGlobalConfig();
        assertEquals("5000", global.get("pollIntervalMs").toString());
        assertEquals("3", global.get("timeoutMultiplier").toString());
        assertEquals("1.5", global.get("deadbandPercent").toString(),
                "a genuinely fractional setting must survive byte-identical");

        // component.instances[] - including nested arrays.
        JsonObject instance = cm.getInstanceConfig("plc-1");
        assertEquals("0", instance.get("slot").toString());
        assertEquals("100", instance.get("rpiMs").toString());
        assertEquals("[50,250,1000]", instance.getAsJsonArray("backoff").toString());
        assertEquals("0.125", instance.get("scale").toString());

        // tags (D-NC5).
        JsonObject tags = cm.getTagConfig().toDict();
        assertEquals("3", tags.get("line").toString());
        assertEquals("\"dallas\"", tags.get("site").toString(),
                "string tags are never coerced");

        // The typed library models read the canonical values.
        assertEquals(30, cm.getHeartbeatConfig().getIntervalSecs());
        assertEquals(9090, cm.getMetricConfig().getPrometheusPort());
    }

    @Test
    void intakeDoesNotMutateTheDocumentItWasHanded() {
        JsonObject raw = greengrassShapedConfig();
        String before = raw.toString();

        manager(raw);

        assertEquals(before, raw.toString(),
                "the provider's document must be left exactly as delivered");
    }

    // ------------------------------------------------------------------
    // The reload path gets the same guarantee
    // ------------------------------------------------------------------

    @Test
    void hotReloadCandidateIsCanonicalizedAndCommitted() {
        ConfigManager cm = manager(greengrassShapedConfig());
        cm.completeInitialization();

        boolean[] notified = {false};
        cm.addConfigChangeListener(() -> { notified[0] = true; return true; });

        // A Greengrass `--update-config` redelivers the whole document as doubles.
        Map<String, Object> updated = greengrassIpcPayload();
        @SuppressWarnings("unchecked")
        Map<String, Object> component = (Map<String, Object>) updated.get("component");
        @SuppressWarnings("unchecked")
        Map<String, Object> global = (Map<String, Object>) component.get("global");
        global.put("pollIntervalMs", 7500.0);
        updated.put("tags", Map.of("site", "dallas"));   // schema types tag values as strings

        assertTrue(cm.tryApplyConfig(asGreengrassProviderWould(updated)),
                "a Greengrass-shaped reload candidate must be accepted");
        assertTrue(notified[0]);
        assertEquals("7500", cm.getGlobalConfig().get("pollIntervalMs").toString());
        assertEquals("30", cm.getFullConfig().getAsJsonObject("heartbeat")
                .get("intervalSecs").toString());
        assertEquals(2, cm.getConfigGeneration());
    }

    @Test
    void candidateValidatorsSeeTheCanonicalDocumentInBothPhases() {
        AtomicReference<String> initialSeen = new AtomicReference<>();
        AtomicReference<String> reloadSeen = new AtomicReference<>();
        ConfigurationCandidateValidator validator = (candidate, current, phase) -> {
            String pollIntervalMs = candidate.getAsJsonObject("component")
                    .getAsJsonObject("global").get("pollIntervalMs").toString();
            if (phase == ConfigurationValidationPhase.INITIAL) {
                initialSeen.set(pollIntervalMs);
            } else {
                reloadSeen.set(pollIntervalMs);
            }
            return ConfigurationCandidateValidator.Result.accept();
        };

        ConfigManager cm = new ConfigManager("com.test.TestComponent", "TestComponent", "gw-01",
                null, greengrassShapedConfig(), null, null,
                List.of(new CandidateValidationRunner.NamedValidator("adapter", validator)),
                Duration.ofSeconds(5));

        Map<String, Object> reload = greengrassIpcPayload();
        reload.put("tags", Map.of("site", "dallas"));
        assertTrue(cm.tryApplyConfig(asGreengrassProviderWould(reload)));

        assertEquals("5000", initialSeen.get(),
                "a component validator must not have to cope with store-specific number encodings");
        assertEquals("5000", reloadSeen.get());
    }

    // ------------------------------------------------------------------
    // Consumer-visible surfaces
    // ------------------------------------------------------------------

    @Test
    void publishedEffectiveConfigIsCanonical() {
        ConfigManager cm = manager(greengrassShapedConfig());

        String published = EffectiveConfigPublisher.redact(cm.getFullConfig()).toString();

        assertTrue(published.contains("\"pollIntervalMs\":5000"), published);
        assertTrue(published.contains("\"intervalSecs\":30"), published);
        assertTrue(published.contains("\"deadbandPercent\":1.5"), published);
        assertTrue(!published.contains("5000.0") && !published.contains("30.0"),
                "the cfg document must not differ per config store: " + published);
    }

    @Test
    void configuredTagsEncodeAsIntegersOnTheWire() throws InvalidProtocolBufferException {
        // D-NC5 / blast radius §5.2(1): an integral tag value must produce the same EcValue kind
        // whatever store delivered the config - IntValue, not DoubleValue on Greengrass only.
        // The protobuf codec types a tag by its decimal scale, so an unrepaired `3.0` would encode
        // DoubleValue while the same logical config on a file/ConfigMap store encodes IntValue.
        //
        // See aNumericTagIsRefusedByTheSchemaGate below: the canonical schema types tag values as
        // strings, so this skew is not reachable through a schema-validated document today. This
        // test pins the intake-boundary guarantee that makes it impossible either way.
        ConfigManager cm = manager(greengrassShapedConfig());

        JsonObject payload = new JsonObject();
        payload.addProperty("ok", true);
        byte[] bytes = MessageBuilder.create("Evt", "1.0")
                .withStructuredPayload(payload)
                .withConfig(cm)
                .build()
                .toBytes();

        Map<String, EcValue> tags = EdgeCommonsMessage.parseFrom(bytes).getTagsMap();
        EcValue line = tags.get("line");
        assertNotNull(line, "the configured tags must reach the envelope");
        assertEquals(EcValue.KindCase.INT_VALUE, line.getKindCase(),
                "an integral tag value must be typed deterministically across platforms");
        assertEquals(3L, line.getIntValue());
        assertEquals(EcValue.KindCase.STRING_VALUE, tags.get("site").getKindCase());
    }

    @Test
    void aNumericTagIsRefusedByTheSchemaGate() {
        // The canonical schema declares `tags` as patternProperties -> {"type":"string"}, so a
        // numeric tag value never survives a schema-validated configuration document, whatever
        // store delivered it. Pinned here because it bounds where the tag-typing skew of §5.2(1)
        // can actually appear: only on paths that bypass the schema gate.
        ConfigManager cm = manager(greengrassShapedConfig());
        cm.completeInitialization();

        // greengrassIpcPayload() carries tags.line = 3.0.
        assertTrue(!cm.tryApplyConfig(asGreengrassProviderWould(greengrassIpcPayload())),
                "a numeric tag value must be rejected by the schema gate");
        assertEquals(1, cm.getConfigGeneration());
    }

    // ------------------------------------------------------------------
    // D-NC2 through the pipeline: loud rejection, never silent truncation
    // ------------------------------------------------------------------

    @Test
    void aFractionalLibraryIntegerFailsLoudlyInsteadOfBeingTruncated() {
        JsonObject config = JsonParser.parseString("""
                {"component":{},"heartbeat":{"intervalSecs":5.5}}""").getAsJsonObject();

        IllegalArgumentException error =
                assertThrows(IllegalArgumentException.class, () -> manager(config));
        assertEquals("configuration value 'heartbeat.intervalSecs' must be a whole number,"
                + " but was 5.5", error.getMessage());
    }

    @Test
    void aNegativeLibraryIntegerFailsLoudlyInsteadOfBeingClamped() {
        JsonObject config = JsonParser.parseString("""
                {"component":{},"heartbeat":{"intervalSecs":-5.0}}""").getAsJsonObject();

        // Canonicalization turns -5.0 into -5; the strict read then refuses it rather than
        // silently substituting the 5 s default.
        IllegalArgumentException error =
                assertThrows(IllegalArgumentException.class, () -> manager(config));
        assertEquals("configuration value 'heartbeat.intervalSecs' must not be negative,"
                + " but was -5", error.getMessage());
    }

    @Test
    void aRejectedReloadKeepsThePreviousGeneration() {
        ConfigManager cm = manager(greengrassShapedConfig());
        cm.completeInitialization();

        // heartbeat.intervalSecs is schema-typed `integer`, so the schema gate refuses the
        // fractional candidate before the strict read is even reached - and the committed
        // generation is untouched either way.
        JsonObject bad = JsonParser.parseString("""
                {"component":{},"heartbeat":{"intervalSecs":5.5}}""").getAsJsonObject();

        assertTrue(!cm.tryApplyConfig(bad), "a fractional integer setting must be rejected");
        assertEquals(1, cm.getConfigGeneration());
        assertEquals(30, cm.getHeartbeatConfig().getIntervalSecs());
    }
}
