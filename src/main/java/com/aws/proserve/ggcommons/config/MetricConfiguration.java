/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.config;

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
    private final static String DEFAULT_MESSAGING_TOPIC = "{ThingName}/{ComponentName}/metric";
    private final static String DEFAULT_CLOUDWATCH_COMPONENT_TOPIC = "cloudwatch/metric/put";
    private final static String DEFAULT_TARGET = "log";
    private final static String DEFAULT_METRIC_NAMESPACE = "ggcommons";
    private final static String DEFAULT_METRIC_FILE_NAME_TEMPLATE = "/greengrass/v2/logs/{ComponentFullName}.metric.log";
    private final static int DEFAULT_INTERVAL_SECS = 5;
    private final static String DEFAULT_MESSAGING_DESTINATION = "ipc";
    private final static String DEFAULT_MAX_FILE_SIZE = "10MB";
    private String target = DEFAULT_TARGET;
    private String namespace = DEFAULT_METRIC_NAMESPACE;
    private String logFileNameTemplate = DEFAULT_METRIC_FILE_NAME_TEMPLATE;
    private String topic;
    private int intervalSecs = DEFAULT_INTERVAL_SECS;
    private String destination = DEFAULT_MESSAGING_DESTINATION;
    private boolean largeFleetWorkaround = false;
    private String maxFileSize = DEFAULT_MAX_FILE_SIZE;

    /**
     * Creates a new metric configuration from a JSON configuration object.
     *
     * @param jsonConfig The JSON object containing metric settings
     */
    MetricConfiguration(JsonObject jsonConfig)
    {
        if (jsonConfig != null)
        {
            if (jsonConfig.has("target"))
                target = jsonConfig.get("target").getAsString();
            if (jsonConfig.has("namespace"))
                namespace = jsonConfig.get("namespace").getAsString();
            if (jsonConfig.has("largeFleetWorkaround"))
                largeFleetWorkaround = jsonConfig.get("largeFleetWorkaround").getAsBoolean();

            if (target.equalsIgnoreCase("log"))
            {
                if (jsonConfig.has("targetConfig"))
                {
                    JsonObject targetConfig = jsonConfig.get("targetConfig").getAsJsonObject();
                    if (targetConfig.has("logFileName"))
                        logFileNameTemplate = targetConfig.get("logFileName").getAsString();
                    if (targetConfig.has("maxFileSize"))
                        maxFileSize = targetConfig.get("maxFileSize").getAsString();
                }
            }

            if (target.equalsIgnoreCase("messaging")) {
                topic = DEFAULT_MESSAGING_TOPIC;
                if (jsonConfig.has("targetConfig"))
                {
                    JsonObject targetConfig = jsonConfig.get("targetConfig").getAsJsonObject();
                    if (targetConfig.has("topic"))
                        topic = targetConfig.get("topic").getAsString();
                    if (targetConfig.has("destination"))
                        destination = targetConfig.get("destination").getAsString();
                }
            }

            if (target.equalsIgnoreCase("cloudwatchcomponent")) {
                topic = DEFAULT_CLOUDWATCH_COMPONENT_TOPIC;
                if (jsonConfig.has("targetConfig"))
                {
                    JsonObject targetConfig = jsonConfig.get("targetConfig").getAsJsonObject();
                    if (targetConfig.has("topic"))
                        topic = targetConfig.get("topic").getAsString();
                }
            }

            if (target.equalsIgnoreCase("cloudwatch")) {
                if (jsonConfig.has("targetConfig"))
                {
                    JsonObject targetConfig = jsonConfig.get("targetConfig").getAsJsonObject();
                    if (targetConfig.has("intervalSecs"))
                        intervalSecs = (targetConfig.get("intervalSecs").getAsBigDecimal()).intValue();
                    if (intervalSecs < 1)
                        intervalSecs = DEFAULT_INTERVAL_SECS;
                }
            }
            LOGGER.debug("Metric configuration: target={}, namespace={}, logFileName={}, topic={}, intervalSecs={}",
                    target, namespace, logFileNameTemplate, topic, intervalSecs);
        }
    }

    public JsonObject toDict() {
        JsonObject retVal = new JsonObject();
        retVal.addProperty("target", target);
        JsonObject targetConfig = new JsonObject();
        switch (target) {
            case "messaging":
                targetConfig.addProperty("topic", topic);
                targetConfig.addProperty("destination", destination);
                break;

            case "cloudwatch":
                targetConfig.addProperty("intervalSecs", topic);
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

    public String getNamespace() {
        return namespace;
    }

    public String getLogFileNameTemplate() {
        return logFileNameTemplate;
    }

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
}
