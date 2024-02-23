package com.aws.proserve.ggcommons.config;

import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

public class MetricConfiguration
{
    protected static final Logger LOGGER = LogManager.getLogger(MetricConfiguration.class);
    private final static String DEFAULT_MESSAGING_TOPIC = "{ThingName}/{ComponentName}/metric";
    private final static String DEFAULT_CLOUDWATCH_COMPONENT_TOPIC = "cloudwatch/metric/put";
    private final static String DEFAULT_TARGET = "log";
    private final static String DEFAULT_METRIC_NAMESPACE = "ggcommons";
    private final static String DEFAULT_METRIC_FILE_NAME_TEMPLATE = "/greengrass/v2/logs/{ComponentName}.metric.log";
    private final static int DEFAULT_INTERVAL_SECS = 5;
    private final static String DEFAULT_MESSAGING_DESTINATION = "ipc";
    private String target = DEFAULT_TARGET;
    private String namespace = DEFAULT_METRIC_NAMESPACE;
    private String logFileNameTemplate = DEFAULT_METRIC_FILE_NAME_TEMPLATE;
    private String topic;
    private int intervalSecs = DEFAULT_INTERVAL_SECS;
    private String destination = DEFAULT_MESSAGING_DESTINATION;

    MetricConfiguration(JsonObject jsonConfig)
    {
        if (jsonConfig != null)
        {
            if (jsonConfig.has("target"))
                target = jsonConfig.get("target").getAsString();
            if (jsonConfig.has("namespace"))
                namespace = jsonConfig.get("namespace").getAsString();

            if (target.equalsIgnoreCase("log"))
            {
                if (jsonConfig.has("targetConfig"))
                {
                    JsonObject targetConfig = jsonConfig.get("targetConfig").getAsJsonObject();
                    if (targetConfig.has("logFileName"))
                        logFileNameTemplate = targetConfig.get("logFileName").getAsString();
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
        return intervalSecs;
    }

    public String getDestination() { return destination; }
}
