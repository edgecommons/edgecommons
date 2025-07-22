/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.config.MetricConfiguration;
import com.aws.proserve.ggcommons.metrics.targets.*;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.*;

/**
 * Provides functionality for emitting metrics from Greengrass components.
 * This class handles metric definitions, buffering, and publishing to configured metric targets.
 */
public class MetricEmitter
{

    protected static final Logger LOGGER = LogManager.getLogger(MetricEmitter.class);

    private static MetricTarget metricTarget = null;

    private static final HashMap<String, Metric> metrics = new HashMap<>();

    private static MetricConfiguration metricConfig;

    private static String thingName;

    private static String componentName;

    /**
     * Initializes the MetricEmitter with configuration settings.
     *
     * @param configManager The configuration manager containing metric settings
     */
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

    /**
     * Returns the current metric configuration.
     *
     * @return The MetricConfiguration instance
     */
    static MetricConfiguration getMetricConfig() {
        return metricConfig;
    }

    /**
     * Returns the name of the AWS IoT thing associated with this component.
     *
     * @return The thing name
     */
    static String getThingName() {
        return thingName;
    }

    /**
     * Returns the name of this Greengrass component.
     *
     * @return The component name
     */
    static String getComponentName() {
        return componentName;
    }

    /**
     * Defines a new metric with its configuration and dimensions.
     *
     * @param metric The metric definition to register
     */
    public static void defineMetric(Metric metric) {
        MetricEmitter.metrics.put(metric.getName(), metric);
    }

    /**
     * Emits metric values for a defined metric. The values will be buffered according to
     * the metric's configuration before being published.
     *
     * @param name The name of the metric to emit
     * @param measureValues Map of measure names to their values
     */
    public static void emitMetric(String name, Map<String, Float> measureValues) {
        if (metrics.containsKey(name)) {
            metricTarget.emitMetric(metrics.get(name), measureValues);
        } else {
            LOGGER.warn("Metric {} is not defined. Ignoring.", name);
        }
    }

    /**
     * Immediately emits metric values without buffering.
     *
     * @param name The name of the metric to emit
     * @param measureValues Map of measure names to their values
     */
    public static void emitMetricNow(String name, Map<String, Float> measureValues) {
        if (metrics.containsKey(name)) {
            metricTarget.emitMetricNow(metrics.get(name), measureValues);
        } else {
            LOGGER.warn("Metric {} is not defined. Ignoring.", name);
        }
    }

}
