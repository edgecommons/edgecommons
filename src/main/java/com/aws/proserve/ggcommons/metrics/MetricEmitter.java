package com.aws.proserve.ggcommons.metrics;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.config.MetricConfiguration;
import com.aws.proserve.ggcommons.metrics.targets.*;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.*;

public class MetricEmitter
{

    protected static final Logger LOGGER = LogManager.getLogger(MetricEmitter.class);

    private static MetricTarget metricTarget = null;

    private static final HashMap<String, Metric> metrics = new HashMap<>();

    private static MetricConfiguration metricConfig;

    private static String thingName;

    private static String componentName;

    public static void init(ConfigManager configManager)
    {
        metricConfig = configManager.getMetricConfig();
        thingName = configManager.getThingName();
        componentName = configManager.getComponentName();
        if (metricTarget == null) {
            String target = metricConfig.getTarget();
            if (target.equalsIgnoreCase("messaging"))
                metricTarget = new Messaging(configManager);
            else if (target.equalsIgnoreCase("log"))
                metricTarget = new Log(configManager);
            else if (target.equalsIgnoreCase("cloudwatch"))
                metricTarget = new CloudWatch(configManager);
            else if (target.equalsIgnoreCase("cloudwatchcomponent"))
                metricTarget = new CloudWatchComponent(configManager);
            else
            {
                LOGGER.warn("Invalid metric target '{}' specified. Defaulting to 'log'", target);
                target = "log";
                metricTarget = new Log(configManager);
            }
            LOGGER.info("MetricEmitter initialized with target: {}", target);
        }
        configManager.addConfigChangeListener(metricTarget);
    }

    static MetricConfiguration getMetricConfig() {
        return metricConfig;
    }

    static String getThingName() {
        return thingName;
    }

    static String getComponentName() {
        return componentName;
    }

    public static void defineMetric(Metric metric) {
        MetricEmitter.metrics.put(metric.getName(), metric);
    }

    public static void emitMetric(String name, Map<String, Float> measureValues) {
        if (metrics.containsKey(name)) {
            metricTarget.emitMetric(metrics.get(name), measureValues);
        } else {
            LOGGER.warn("Metric {} is not defined. Ignoring.", name);
        }
    }

    public static void emitMetricNow(String name, Map<String, Float> measureValues) {
        if (metrics.containsKey(name)) {
            metricTarget.emitMetricNow(metrics.get(name), measureValues);
        } else {
            LOGGER.warn("Metric {} is not defined. Ignoring.", name);
        }
    }

}
