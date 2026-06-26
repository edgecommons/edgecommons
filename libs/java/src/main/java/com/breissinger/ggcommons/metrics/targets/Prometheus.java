/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.metrics.targets;

import com.breissinger.ggcommons.config.ConfigManager;
import com.breissinger.ggcommons.metrics.Metric;
import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;
import io.prometheus.client.CollectorRegistry;
import io.prometheus.client.Gauge;
import io.prometheus.client.exporter.common.TextFormat;

import java.io.IOException;
import java.io.OutputStream;
import java.io.StringWriter;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.TreeMap;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.Executors;

/**
 * The pull-based {@code prometheus} metric target (FR-MET-1/2/3) — the default on the KUBERNETES
 * platform. Unlike every other {@link MetricTarget} (log/messaging/cloudwatch/cloudwatchcomponent),
 * which <em>push</em> on each emit, this target <b>inverts the lifecycle (FR-MET-2)</b>:
 *
 * <ul>
 *   <li>{@link #emitMetric}/{@link #emitMetricNow} only <b>update an in-process registry</b> (they do
 *       not push anywhere);</li>
 *   <li>{@link #flush()} is a <b>no-op</b> w.r.t. delivery — a Prometheus scrape pulls the current
 *       values;</li>
 *   <li>{@link #close()} <b>stops the HTTP listener</b>, releasing the port and its daemon threads.</li>
 * </ul>
 *
 * <p>The registry is served as OpenMetrics/Prometheus text at the configured {@code path} (default
 * {@code /metrics}) on the configured {@code port} (default {@code 9090}), bound on {@code 0.0.0.0} so
 * a kubelet/Prometheus scraper across the pod network can reach it. The exposition is written by the
 * official client's {@link TextFormat#write004} writer, and the response carries
 * {@link TextFormat#CONTENT_TYPE_004} as the {@code Content-Type} — Prometheus 3.x rejects a
 * missing/blank type. Built on the JDK's {@link com.sun.net.httpserver.HttpServer} (the same minimal,
 * framework-free pattern as {@code HealthServer}); the prometheus client supplies the registry + the
 * exposition format only.
 *
 * <h2>Dimension &rarr; label mapping (FR-MET-3 — locked for four-way parity)</h2>
 * For each measure in an emitted metric a {@link Gauge} is registered/updated (latest-value semantics —
 * a scrape reads the current value):
 * <ul>
 *   <li><b>gauge name</b> = {@code sanitize(lowercase("{namespace}_{measureName}"))} where the
 *       namespace defaults to {@code "ggcommons"}; {@link #sanitizeMetricName} replaces every character
 *       not matching {@code [a-z0-9_]} with {@code '_'} and prefixes {@code '_'} if the result starts
 *       with a digit (Prometheus metric-name rules);</li>
 *   <li><b>labels</b> = the metric's dimensions ({@link Metric#getDimensions()}, which already include
 *       {@code category}/{@code coreName}/{@code component} plus any custom dimensions). Each label
 *       <em>name</em> is sanitized to {@code [a-zA-Z_][a-zA-Z0-9_]*} by {@link #sanitizeLabelName}
 *       (invalid chars &rarr; {@code '_'}, prefix {@code '_'} if starting with a digit; case is
 *       preserved); the label <em>value</em> is used as-is;</li>
 *   <li>the gauge for that label-set is <b>set</b> to the measure's float value on each emit.</li>
 * </ul>
 *
 * <p>Label names are positional in the prometheus client, so a gauge is registered once with the
 * dimension keys sorted deterministically and subsequent emits supply values in the same order. If the
 * same gauge name is later emitted with a <em>different</em> label-name set (an unusual case — the same
 * measure name carrying different dimensions), that emit is logged and skipped rather than throwing.
 *
 * <p>Each instance owns a private {@link CollectorRegistry} (not the global default), so multiple
 * components/instances and tests never share registry state and {@link #close()} fully releases it.
 */
public final class Prometheus extends MetricTarget
{
    private final CollectorRegistry registry;
    private final HttpServer server;
    private final int port;
    private final String path;
    /** Gauge cache keyed by sanitized gauge name; the holder records the ordered label-name list. */
    private final Map<String, RegisteredGauge> gauges = new ConcurrentHashMap<>();

    /** A registered gauge together with the ordered (sorted) sanitized label names it was built with. */
    private record RegisteredGauge(Gauge gauge, List<String> labelNames) {}

    /**
     * Creates the target: a fresh registry plus an HTTP server bound on {@code 0.0.0.0:<port>} serving
     * the exposition at {@code path}. A bind/start failure is logged and swallowed (mirroring the health
     * server) so a port conflict never crashes the component — emits still update the registry; only the
     * scrape endpoint is unavailable.
     *
     * @param configManager the configuration manager supplying namespace + prometheus port/path
     */
    public Prometheus(ConfigManager configManager)
    {
        super(configManager);
        this.registry = new CollectorRegistry();
        this.path = metricConfig.getPrometheusPath();
        int requestedPort = metricConfig.getPrometheusPort();

        HttpServer started = null;
        int boundPort = requestedPort;
        try
        {
            started = HttpServer.create(new InetSocketAddress("0.0.0.0", requestedPort), 0);
            started.createContext("/", this::handle);
            started.setExecutor(Executors.newSingleThreadExecutor(runnable -> {
                Thread thread = new Thread(runnable, "ggcommons-prometheus");
                thread.setDaemon(true);
                return thread;
            }));
            started.start();
            boundPort = started.getAddress().getPort();
            LOGGER.info("Prometheus metric target listening on 0.0.0.0:{}{}", boundPort, path);
        }
        catch (IOException e)
        {
            LOGGER.error("Failed to start Prometheus metric endpoint on port {} (continuing without "
                    + "the scrape endpoint; emits still update the registry): {}", requestedPort, e.getMessage());
        }
        this.server = started;
        this.port = boundPort;
    }

    @Override
    public void emitMetric(Metric metric, Map<String, Float> measureValues)
    {
        // Pull-based: emit only updates the registry; no push (FR-MET-2).
        updateRegistry(metric, measureValues);
    }

    @Override
    public void emitMetricNow(Metric metric, Map<String, Float> measureValues)
    {
        // Identical to emitMetric for the pull model — there is nothing to "push now"; the scrape pulls.
        updateRegistry(metric, measureValues);
    }

    /**
     * Updates the in-process registry for one emitted metric: one gauge per measure, the metric's
     * dimensions as labels (FR-MET-3). Latest-value semantics — the gauge is set to the new value.
     */
    private void updateRegistry(Metric metric, Map<String, Float> measureValues)
    {
        if (measureValues == null || measureValues.isEmpty())
        {
            return;
        }
        String namespace = metricConfig.getNamespace();
        // Deterministic label ordering: sort dimension keys, sanitize names, align values to that order.
        Map<String, String> dimensions = metric.getDimensions();
        TreeMap<String, String> sorted = new TreeMap<>(dimensions == null ? Map.of() : dimensions);
        List<String> labelNames = new ArrayList<>(sorted.size());
        List<String> labelValuesList = new ArrayList<>(sorted.size());
        for (Map.Entry<String, String> e : sorted.entrySet())
        {
            labelNames.add(sanitizeLabelName(e.getKey()));
            labelValuesList.add(e.getValue() == null ? "" : e.getValue());
        }
        String[] labelValues = labelValuesList.toArray(new String[0]);

        for (Map.Entry<String, Float> measure : measureValues.entrySet())
        {
            if (measure.getValue() == null)
            {
                continue;
            }
            String gaugeName = sanitizeMetricName(namespace + "_" + measure.getKey());
            RegisteredGauge rg = gauges.computeIfAbsent(gaugeName, name -> registerGauge(name, labelNames));
            if (!rg.labelNames().equals(labelNames))
            {
                LOGGER.warn("Prometheus gauge '{}' already registered with labels {} but this emit has "
                        + "labels {}; skipping (a gauge's label set is fixed at first registration)",
                        gaugeName, rg.labelNames(), labelNames);
                continue;
            }
            rg.gauge().labels(labelValues).set(measure.getValue());
        }
    }

    /** Registers a gauge with the given sanitized label names against this instance's registry. */
    private RegisteredGauge registerGauge(String name, List<String> labelNames)
    {
        Gauge gauge = Gauge.build()
                .name(name)
                .help("ggcommons metric " + name)
                .labelNames(labelNames.toArray(new String[0]))
                .register(registry);
        return new RegisteredGauge(gauge, labelNames);
    }

    /**
     * Serves a single request: {@code GET <path>} &rarr; 200 with the OpenMetrics exposition + a valid
     * {@code Content-Type}; the configured path with a non-GET method &rarr; 405; any other path &rarr; 404.
     */
    private void handle(HttpExchange exchange) throws IOException
    {
        try
        {
            String requestPath = exchange.getRequestURI().getPath();
            String method = exchange.getRequestMethod();

            if (!path.equals(requestPath))
            {
                byte[] body = "not found".getBytes(StandardCharsets.UTF_8);
                exchange.getResponseHeaders().set("Content-Type", "text/plain; charset=utf-8");
                exchange.sendResponseHeaders(404, body.length);
                writeBody(exchange, body);
                return;
            }
            if (!"GET".equalsIgnoreCase(method))
            {
                byte[] body = "method not allowed".getBytes(StandardCharsets.UTF_8);
                exchange.getResponseHeaders().set("Content-Type", "text/plain; charset=utf-8");
                exchange.sendResponseHeaders(405, body.length);
                writeBody(exchange, body);
                return;
            }
            // The client's exposition writer (write004) sets the Prometheus text format; advertise the
            // matching Content-Type (Prometheus 3.x rejects a missing/blank type).
            StringWriter writer = new StringWriter();
            TextFormat.write004(writer, registry.metricFamilySamples());
            byte[] body = writer.toString().getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().set("Content-Type", TextFormat.CONTENT_TYPE_004);
            exchange.sendResponseHeaders(200, body.length);
            writeBody(exchange, body);
        }
        finally
        {
            exchange.close();
        }
    }

    private static void writeBody(HttpExchange exchange, byte[] body) throws IOException
    {
        try (OutputStream os = exchange.getResponseBody())
        {
            os.write(body);
        }
    }

    /**
     * Sanitizes a Prometheus <em>metric name</em>: lowercases, replaces every character not matching
     * {@code [a-z0-9_]} with {@code '_'}, and prefixes {@code '_'} if the result starts with a digit.
     *
     * @param raw the raw {@code "{namespace}_{measureName}"} string
     * @return a valid Prometheus metric name
     */
    static String sanitizeMetricName(String raw)
    {
        if (raw == null || raw.isEmpty())
        {
            return "_";
        }
        String lower = raw.toLowerCase(Locale.ROOT);
        StringBuilder sb = new StringBuilder(lower.length());
        for (int i = 0; i < lower.length(); i++)
        {
            char c = lower.charAt(i);
            boolean ok = (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '_';
            sb.append(ok ? c : '_');
        }
        char first = sb.charAt(0);
        if (first >= '0' && first <= '9')
        {
            sb.insert(0, '_');
        }
        return sb.toString();
    }

    /**
     * Sanitizes a Prometheus <em>label name</em>: replaces every character not matching
     * {@code [a-zA-Z0-9_]} with {@code '_'} and prefixes {@code '_'} if the result starts with a digit.
     * Case is preserved (unlike metric names, label names are not lowercased).
     *
     * @param raw the raw dimension key
     * @return a valid Prometheus label name
     */
    static String sanitizeLabelName(String raw)
    {
        if (raw == null || raw.isEmpty())
        {
            return "_";
        }
        StringBuilder sb = new StringBuilder(raw.length());
        for (int i = 0; i < raw.length(); i++)
        {
            char c = raw.charAt(i);
            boolean ok = (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '_';
            sb.append(ok ? c : '_');
        }
        char first = sb.charAt(0);
        if (first >= '0' && first <= '9')
        {
            sb.insert(0, '_');
        }
        return sb.toString();
    }

    /**
     * Returns the actual bound TCP port (equals the requested port, or the ephemeral port chosen by the
     * OS when {@code 0} was requested; equals the requested port if the bind failed). Useful for tests.
     *
     * @return the bound port
     */
    public int getPort()
    {
        return port;
    }

    @Override
    public boolean onConfigurationChanged()
    {
        // The registry and the bound endpoint persist across config changes; port/path are not
        // hot-reconfigured (a port change needs a target rebuild). Nothing to reset for the pull model.
        LOGGER.debug("Configuration changed; Prometheus target keeps its registry and listener");
        return true;
    }

    /** Stops the HTTP listener (releasing the port + daemon threads). Idempotent; safe during shutdown. */
    @Override
    public void close()
    {
        if (server != null)
        {
            try
            {
                server.stop(0);
                LOGGER.debug("Prometheus metric endpoint on port {} stopped", port);
            }
            catch (Exception e)
            {
                LOGGER.warn("Error stopping Prometheus metric endpoint: {}", e.getMessage());
            }
        }
    }
}
