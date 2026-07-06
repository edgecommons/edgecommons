/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

/**
 * Configuration class for managing metric collection and reporting settings.
 * Defines how metrics are collected, buffered, and published to various targets.
 */
public class MetricConfiguration
{
    protected static final Logger LOGGER = LogManager.getLogger(MetricConfiguration.class);
    private final static String DEFAULT_CLOUDWATCH_COMPONENT_TOPIC = "cloudwatch/metric/put";
    private final static String DEFAULT_TARGET = "log";
    private final static String DEFAULT_METRIC_NAMESPACE = "edgecommons";
    private final static String DEFAULT_METRIC_FILE_NAME_TEMPLATE = "/greengrass/v2/logs/{ComponentFullName}.metric.log";
    private final static int DEFAULT_INTERVAL_SECS = 5;
    private final static String DEFAULT_MESSAGING_DESTINATION = "ipc";
    private final static String DEFAULT_MAX_FILE_SIZE = "10MB";
    /** Default HTTP port for the prometheus target's /metrics endpoint (matches the canonical schema). */
    private final static int DEFAULT_PROMETHEUS_PORT = 9090;
    /** Default HTTP path for the prometheus target's OpenMetrics exposition (matches the schema). */
    private final static String DEFAULT_PROMETHEUS_PATH = "/metrics";
    private String target = DEFAULT_TARGET;
    /**
     * Whether {@code metricEmission.target} was explicitly present in the config (vs defaulted). The
     * top tier of the metric-target precedence (FR-RT-3): when {@code false}, the platform-profile
     * default (prometheus on KUBERNETES) applies; see {@code MetricEmitter.resolveEffectiveTarget}.
     */
    private boolean targetExplicitlySet = false;
    private String namespace = DEFAULT_METRIC_NAMESPACE;
    private String logFileNameTemplate = DEFAULT_METRIC_FILE_NAME_TEMPLATE;
    /**
     * The raw {@code metricEmission.targetConfig.logFileName} exactly as configured, or {@code null}
     * when absent. Distinguishing "absent" from an explicit value lets the metric {@code log} target
     * apply the HOST-aware path precedence (explicit ▸ platform-profile default ▸ library default) —
     * mirroring {@link #targetExplicitlySet} for the target.
     */
    private String explicitLogFileName = null;
    private String topic;
    private int intervalSecs = DEFAULT_INTERVAL_SECS;
    private String destination = DEFAULT_MESSAGING_DESTINATION;
    private boolean largeFleetWorkaround = false;
    private String maxFileSize = DEFAULT_MAX_FILE_SIZE;
    private int prometheusPort = DEFAULT_PROMETHEUS_PORT;
    private String prometheusPath = DEFAULT_PROMETHEUS_PATH;
    private BufferConfiguration bufferConfig = BufferConfiguration.memory();

    /**
     * Creates a new metric configuration from a JSON configuration object.
     *
     * @param jsonConfig The JSON object containing metric settings
     */
    MetricConfiguration(JsonObject jsonConfig)
    {
        if (jsonConfig != null)
        {
            if (jsonConfig.has("target")) {
                target = jsonConfig.get("target").getAsString();
                targetExplicitlySet = true;
            }
            if (jsonConfig.has("namespace"))
                namespace = jsonConfig.get("namespace").getAsString();
            if (jsonConfig.has("largeFleetWorkaround"))
                largeFleetWorkaround = jsonConfig.get("largeFleetWorkaround").getAsBoolean();

            if (target.equalsIgnoreCase("log"))
            {
                if (jsonConfig.has("targetConfig"))
                {
                    JsonObject targetConfig = jsonConfig.get("targetConfig").getAsJsonObject();
                    if (targetConfig.has("logFileName")) {
                        explicitLogFileName = targetConfig.get("logFileName").getAsString();
                        logFileNameTemplate = explicitLogFileName;
                    }
                    if (targetConfig.has("maxFileSize"))
                        maxFileSize = targetConfig.get("maxFileSize").getAsString();
                }
            }

            // UNS-CANONICAL-DESIGN §4.3 / D-U9: the messaging target's topic is no longer
            // configurable (targetConfig.topic is removed from the schema) — the Messaging target
            // builds the UNS metric topic ecv1/{device}/{component}/main/metric/{metricName}
            // itself. Only the destination survives.
            if (target.equalsIgnoreCase("messaging")) {
                if (jsonConfig.has("targetConfig"))
                {
                    JsonObject targetConfig = jsonConfig.get("targetConfig").getAsJsonObject();
                    if (targetConfig.has("destination"))
                        destination = targetConfig.get("destination").getAsString();
                }
            }

            // The cloudwatchcomponent topic is the external AWS Greengrass component contract
            // (cloudwatch/metric/put, D-U21) — fixed, no override.
            if (target.equalsIgnoreCase("cloudwatchcomponent")) {
                topic = DEFAULT_CLOUDWATCH_COMPONENT_TOPIC;
            }

            if (target.equalsIgnoreCase("cloudwatch")) {
                JsonObject bufferJson = null;
                if (jsonConfig.has("targetConfig"))
                {
                    JsonObject targetConfig = jsonConfig.get("targetConfig").getAsJsonObject();
                    if (targetConfig.has("intervalSecs"))
                        intervalSecs = (targetConfig.get("intervalSecs").getAsBigDecimal()).intValue();
                    if (intervalSecs < 1)
                        intervalSecs = DEFAULT_INTERVAL_SECS;
                    if (targetConfig.has("buffer") && targetConfig.get("buffer").isJsonObject())
                        bufferJson = targetConfig.getAsJsonObject("buffer");
                }
                // Default for the cloudwatch target is a durable, disk-backed store-and-forward
                // buffer (survives lengthy disconnects); type=memory keeps the in-memory batching.
                bufferConfig = BufferConfiguration.fromJson(bufferJson);
            }

            // Prometheus target (port/path) — read unconditionally when present so the KUBERNETES
            // profile default (target defaulted to prometheus, not set in config) still honors an
            // explicit targetConfig.port/path; both have schema defaults (9090, /metrics) otherwise.
            if (jsonConfig.has("targetConfig"))
            {
                JsonObject targetConfig = jsonConfig.get("targetConfig").getAsJsonObject();
                if (targetConfig.has("port"))
                    prometheusPort = targetConfig.get("port").getAsBigDecimal().intValue();
                if (targetConfig.has("path"))
                    prometheusPath = targetConfig.get("path").getAsString();
            }
            LOGGER.debug("Metric configuration: target={}, namespace={}, logFileName={}, topic={}, "
                    + "intervalSecs={}, prometheusPort={}, prometheusPath={}",
                    target, namespace, logFileNameTemplate, topic, intervalSecs, prometheusPort, prometheusPath);
        }
    }

    public JsonObject toDict() {
        JsonObject retVal = new JsonObject();
        retVal.addProperty("target", target);
        JsonObject targetConfig = new JsonObject();
        switch (target) {
            case "messaging":
                targetConfig.addProperty("destination", destination);
                break;

            case "cloudwatch":
                targetConfig.addProperty("intervalSecs", intervalSecs);
                break;

            case "log":
                targetConfig.addProperty("filename", logFileNameTemplate);
                targetConfig.addProperty("maxFileSize", maxFileSize);
                break;
        }
        retVal.add("targetConfig", targetConfig);
        return retVal;
    }

    @Override
    public String toString() {
        return toDict().toString();
    }

    public String getTarget() {
        return target;
    }

    /**
     * Whether {@code metricEmission.target} was explicitly set in the config (vs library-defaulted to
     * {@code "log"}). Used by {@code MetricEmitter} to apply the platform-profile metric-target default
     * (prometheus on KUBERNETES) only when the config did not specify a target (FR-RT-3 precedence).
     *
     * @return {@code true} if the config explicitly set {@code metricEmission.target}
     */
    public boolean isTargetExplicitlySet() {
        return targetExplicitlySet;
    }

    /**
     * The HTTP port for the prometheus target's {@code /metrics} endpoint (prometheus target only).
     *
     * @return the configured port, or the default {@value #DEFAULT_PROMETHEUS_PORT}
     */
    public int getPrometheusPort() {
        return prometheusPort;
    }

    /**
     * The HTTP path for the prometheus target's OpenMetrics exposition (prometheus target only).
     *
     * @return the configured path, or the default {@value #DEFAULT_PROMETHEUS_PATH}
     */
    public String getPrometheusPath() {
        return prometheusPath;
    }

    public String getNamespace() {
        return namespace;
    }

    public String getLogFileNameTemplate() {
        return logFileNameTemplate;
    }

    /**
     * The raw {@code metricEmission.targetConfig.logFileName} exactly as configured, or {@code null}
     * when absent. Lets the metric {@code log} target distinguish an explicit path (which must win)
     * from an unset one (which falls through to the platform-profile default, then the library
     * default) — mirroring {@link #isTargetExplicitlySet()} for the target.
     *
     * @return the explicit log-file template, or {@code null} if not configured
     */
    public String getExplicitLogFileName() {
        return explicitLogFileName;
    }

    /**
     * The fixed topic of the {@code cloudwatchcomponent} target ({@code cloudwatch/metric/put},
     * the external AWS Greengrass component contract — D-U21), or {@code null} for every other
     * target. The {@code messaging} target no longer carries a configured topic: it publishes to
     * the UNS metric topic {@code ecv1/{device}/{component}/main/metric/{metricName}} (§4.3).
     */
    public String getTopic() {
        return topic;
    }

    public int getIntervalSecs() {
        // amazonq-ignore-next-line
        return intervalSecs;
    }

    public String getDestination() { return destination; }

    public boolean getLargeFleetWorkaround() { return largeFleetWorkaround; }

    public String getMaxFileSize() { return maxFileSize; }

    /** The cloudwatch durable/in-memory buffer settings (memory by default for non-cloudwatch targets). */
    public BufferConfiguration getBufferConfig() { return bufferConfig; }
}
