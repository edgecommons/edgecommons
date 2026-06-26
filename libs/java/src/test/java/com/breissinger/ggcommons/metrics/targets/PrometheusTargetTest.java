/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.metrics.targets;

import com.breissinger.ggcommons.config.ConfigurationFactory;
import com.breissinger.ggcommons.config.MetricConfiguration;
import com.breissinger.ggcommons.metrics.Metric;
import com.breissinger.ggcommons.metrics.MetricBuilder;
import com.breissinger.ggcommons.test.MockConfigurationService;
import com.sun.net.httpserver.HttpServer;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.util.HashMap;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for the pull-based {@link Prometheus} metric target (FR-MET-1/2/3): registry update on
 * emit, OpenMetrics exposition over a real loopback HTTP GET (valid Content-Type), the inverted
 * lifecycle (flush no-op, close stops the listener), and the locked dimension&rarr;label + name
 * sanitization policy.
 */
class PrometheusTargetTest {

    /** Config returning a caller-supplied metric configuration (mirrors MetricEmitterTest). */
    private static class PromConfig extends MockConfigurationService {
        private final MetricConfiguration metricConfig;

        PromConfig(String metricJson) {
            JsonObject root = new JsonObject();
            root.add("metricEmission", JsonParser.parseString(metricJson).getAsJsonObject());
            this.metricConfig = ConfigurationFactory.createMetricConfiguration(root);
        }

        @Override
        public MetricConfiguration getMetricConfig() {
            return metricConfig;
        }
    }

    private static Prometheus targetOnEphemeralPort(String namespace) {
        // port 0 => bind an ephemeral port; getPort() returns the actual bound port.
        return new Prometheus(new PromConfig(
                "{\"target\":\"prometheus\",\"namespace\":\"" + namespace + "\",\"targetConfig\":{\"port\":0}}"));
    }

    private static Metric metric(String name) {
        return MetricBuilder.create(name)
                .withNamespace("ignored-uses-config-namespace")
                .withThingName("thing-1")
                .withComponentName("comp-A")
                .addMeasure("value", "Count", 60)
                .build();
    }

    private static Map<String, Float> values(float v) {
        Map<String, Float> m = new HashMap<>();
        m.put("value", v);
        return m;
    }

    private static HttpResponse<String> get(int port, String path) throws Exception {
        HttpClient client = HttpClient.newHttpClient();
        return client.send(
                HttpRequest.newBuilder().uri(URI.create("http://127.0.0.1:" + port + path)).GET().build(),
                HttpResponse.BodyHandlers.ofString());
    }

    @Test
    void emitUpdatesRegistryAndMetricsServesOpenMetrics() throws Exception {
        Prometheus target = targetOnEphemeralPort("ns1");
        try {
            assertTrue(target.getPort() > 0, "ephemeral port should be bound");
            target.emitMetric(metric("m1"), values(7.0f));

            HttpResponse<String> resp = get(target.getPort(), "/metrics");
            assertEquals(200, resp.statusCode());

            // Prometheus 3.x rejects a blank Content-Type — the client's exposition writer sets it.
            String contentType = resp.headers().firstValue("content-type").orElse("");
            assertFalse(contentType.isBlank(), "Content-Type must not be blank");
            assertTrue(contentType.contains("text/plain"), "Content-Type was: " + contentType);

            String body = resp.body();
            // gauge name = sanitize(lowercase("ns1_value")) = "ns1_value"; dimensions become labels.
            assertTrue(body.contains("# TYPE ns1_value gauge"), body);
            assertTrue(body.contains("ns1_value{"), body);
            assertTrue(body.contains("category=\"m1\""), body);
            assertTrue(body.contains("component=\"comp-A\""), body);
            assertTrue(body.contains("coreName=\"thing-1\""), body);
            assertTrue(body.contains("7.0"), body);
        } finally {
            target.close();
        }
    }

    @Test
    void emitNowAlsoUpdatesRegistryAndLatestValueWins() throws Exception {
        Prometheus target = targetOnEphemeralPort("ns1");
        try {
            target.emitMetricNow(metric("m1"), values(1.0f));
            target.emitMetricNow(metric("m1"), values(42.0f)); // latest-value gauge semantics

            String body = get(target.getPort(), "/metrics").body();
            assertTrue(body.contains("42.0"), body);
            assertFalse(body.contains(" 1.0\n") || body.contains(" 1.0\r"),
                    "older value should be overwritten by the latest emit");
        } finally {
            target.close();
        }
    }

    @Test
    void flushIsANoOpAndDoesNotThrow() {
        Prometheus target = targetOnEphemeralPort("ns1");
        try {
            target.emitMetric(metric("m1"), values(3.0f));
            assertDoesNotThrow(target::flush); // no delivery; the scrape pulls
        } finally {
            target.close();
        }
    }

    @Test
    void unknownPathReturns404AndNonGetReturns405() throws Exception {
        Prometheus target = targetOnEphemeralPort("ns1");
        try {
            assertEquals(404, get(target.getPort(), "/nope").statusCode());

            HttpClient client = HttpClient.newHttpClient();
            HttpResponse<String> post = client.send(
                    HttpRequest.newBuilder().uri(URI.create("http://127.0.0.1:" + target.getPort() + "/metrics"))
                            .POST(HttpRequest.BodyPublishers.noBody()).build(),
                    HttpResponse.BodyHandlers.ofString());
            assertEquals(405, post.statusCode());
        } finally {
            target.close();
        }
    }

    @Test
    void customPathIsServed() throws Exception {
        Prometheus target = new Prometheus(new PromConfig(
                "{\"target\":\"prometheus\",\"namespace\":\"ns1\",\"targetConfig\":{\"port\":0,\"path\":\"/custommetrics\"}}"));
        try {
            target.emitMetric(metric("m1"), values(5.0f));
            assertEquals(200, get(target.getPort(), "/custommetrics").statusCode());
            assertEquals(404, get(target.getPort(), "/metrics").statusCode());
        } finally {
            target.close();
        }
    }

    @Test
    void closeStopsListenerAndReleasesPort() throws Exception {
        Prometheus target = targetOnEphemeralPort("ns1");
        int port = target.getPort();
        target.emitMetric(metric("m1"), values(1.0f));
        target.close();

        // The port must be free again: a new server can bind it (deterministic proof of release).
        HttpServer rebind = HttpServer.create(new InetSocketAddress("0.0.0.0", port), 0);
        assertEquals(port, rebind.getAddress().getPort());
        rebind.stop(0);
    }

    @Test
    void closeIsIdempotent() {
        Prometheus target = targetOnEphemeralPort("ns1");
        target.close();
        assertDoesNotThrow(target::close);
    }

    @Test
    void bindFailureIsSwallowedAndEmitStillUpdatesRegistry() throws Exception {
        // First target holds the port; a second on the SAME port fails to bind and is swallowed.
        Prometheus first = targetOnEphemeralPort("ns1");
        int port = first.getPort();
        try {
            Prometheus second = new Prometheus(new PromConfig(
                    "{\"target\":\"prometheus\",\"namespace\":\"ns2\",\"targetConfig\":{\"port\":" + port + "}}"));
            // No throw despite the bind failure; getPort() reports the requested port.
            assertEquals(port, second.getPort());
            // Emit still updates the (in-process) registry even with no listener; no external delivery.
            assertDoesNotThrow(() -> second.emitMetric(metric("m1"), values(9.0f)));
            assertDoesNotThrow(second::flush);
            assertDoesNotThrow(second::close);
        } finally {
            first.close();
        }
    }

    @Test
    void onConfigurationChangedReturnsTrue() {
        Prometheus target = targetOnEphemeralPort("ns1");
        try {
            assertTrue(target.onConfigurationChanged());
        } finally {
            target.close();
        }
    }

    @Test
    void emptyMeasuresEmitDoesNothing() throws Exception {
        Prometheus target = targetOnEphemeralPort("ns1");
        try {
            assertDoesNotThrow(() -> target.emitMetric(metric("m1"), new HashMap<>()));
            // Registry has no gauge yet => the exposition has no ns1_value line.
            assertFalse(get(target.getPort(), "/metrics").body().contains("ns1_value{"));
        } finally {
            target.close();
        }
    }

    @Test
    void mismatchedLabelSetForSameGaugeIsSkipped() throws Exception {
        Prometheus target = targetOnEphemeralPort("ns1");
        try {
            // First emit registers gauge "ns1_value" with labels {category, component, coreName}.
            target.emitMetric(metric("m1"), values(1.0f));
            // A second metric with the SAME measure name but an extra custom dimension => different
            // label set for the same gauge name. It must be skipped (logged), not throw.
            Metric extra = MetricBuilder.create("m1")
                    .withNamespace("x")
                    .withThingName("thing-1")
                    .withComponentName("comp-A")
                    .addDimension("region", "us")
                    .addMeasure("value", "Count", 60)
                    .build();
            assertDoesNotThrow(() -> target.emitMetric(extra, values(2.0f)));
            // The original gauge value remains scrapeable.
            assertTrue(get(target.getPort(), "/metrics").body().contains("ns1_value{"));
        } finally {
            target.close();
        }
    }

    // ---------- sanitization (FR-MET-3, locked for parity) ----------

    @Test
    void sanitizeMetricNameLowercasesAndReplacesInvalidChars() {
        assertEquals("my_app_metrics_cpu_load",
                Prometheus.sanitizeMetricName("My.App/Metrics_CPU%load"));
    }

    @Test
    void sanitizeMetricNamePrefixesLeadingDigit() {
        assertEquals("_1ns_value", Prometheus.sanitizeMetricName("1ns_value"));
    }

    @Test
    void sanitizeMetricNameHandlesNullAndEmpty() {
        assertEquals("_", Prometheus.sanitizeMetricName(null));
        assertEquals("_", Prometheus.sanitizeMetricName(""));
    }

    @Test
    void sanitizeLabelNamePreservesCaseAndReplacesInvalidChars() {
        // Label names keep case (unlike metric names) and allow [a-zA-Z0-9_].
        assertEquals("weird_Key_1", Prometheus.sanitizeLabelName("weird-Key.1"));
        assertEquals("coreName", Prometheus.sanitizeLabelName("coreName"));
    }

    @Test
    void sanitizeLabelNamePrefixesLeadingDigit() {
        assertEquals("_9bad", Prometheus.sanitizeLabelName("9bad"));
    }

    @Test
    void sanitizeLabelNameHandlesNullAndEmpty() {
        assertEquals("_", Prometheus.sanitizeLabelName(null));
        assertEquals("_", Prometheus.sanitizeLabelName(""));
    }

    @Test
    void hostileDimensionsAndMeasureAreSanitizedInExposition() throws Exception {
        Prometheus target = new Prometheus(new PromConfig(
                "{\"target\":\"prometheus\",\"namespace\":\"My.App\",\"targetConfig\":{\"port\":0}}"));
        try {
            Metric m = MetricBuilder.create("m1")
                    .withNamespace("x")
                    .withThingName("thing-1")
                    .withComponentName("comp-A")
                    .addDimension("weird-key.1", "v1")
                    .addMeasure("cpu%load", "Percent", 60)
                    .build();
            Map<String, Float> vals = new HashMap<>();
            vals.put("cpu%load", 0.5f);
            target.emitMetric(m, vals);

            String body = get(target.getPort(), "/metrics").body();
            // namespace "My.App" + measure "cpu%load" => lowercased + sanitized "my_app_cpu_load".
            assertTrue(body.contains("my_app_cpu_load{"), body);
            // label name "weird-key.1" => "weird_key_1"; value used as-is.
            assertTrue(body.contains("weird_key_1=\"v1\""), body);
        } finally {
            target.close();
        }
    }
}
