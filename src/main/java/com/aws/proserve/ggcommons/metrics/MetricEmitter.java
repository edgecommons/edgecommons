/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.config.MetricConfiguration;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.aws.proserve.ggcommons.metrics.targets.*;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.*;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Provides functionality for emitting metrics from Greengrass components.
 * This class handles metric definitions, buffering, and publishing to configured metric targets.
 */
public class MetricEmitter
{
    protected static final Logger LOGGER = LogManager.getLogger(MetricEmitter.class);



    private final MetricTarget metricTarget;
    private final ConcurrentHashMap<String, Metric> metrics = new ConcurrentHashMap<>();
    private final MetricConfiguration metricConfig;
    private final String thingName;
    private final String componentName;


    /**
     * Protected no-arg constructor for testing/subclassing (e.g. mock metric services).
     */
    protected MetricEmitter() {
        this.metricTarget = null;
        this.metricConfig = null;
        this.thingName = null;
        this.componentName = null;
    }

    /**
     * Package-private constructor for builder pattern.
     */
    MetricEmitter(ConfigManager configurationService, MessagingClient messagingService) {
        this.metricConfig = configurationService.getMetricConfig();
        this.thingName = configurationService.getThingName();
        this.componentName = configurationService.getComponentName();
        
        String target = metricConfig.getTarget();
        this.metricTarget = switch (target.toLowerCase()) {
            case "messaging" -> {
                Messaging messaging = new Messaging(configurationService);
                if (messagingService != null) {
                    messaging.setMessagingService(messagingService);
                }
                yield messaging;
            }
            case "cloudwatch" -> new CloudWatch(configurationService);
            case "cloudwatchcomponent" -> {
                CloudWatchComponent cwComponent = new CloudWatchComponent(configurationService);
                if (messagingService != null) {
                    cwComponent.setMessagingService(messagingService);
                }
                yield cwComponent;
            }
            case "log" -> new Log(configurationService);
            default -> {
                LOGGER.warn("Invalid metric target '{}' specified. Defaulting to 'log'", target);
                yield new Log(configurationService);
            }
        };
        
        LOGGER.info("MetricEmitter initialized with target: {}", target);
        configurationService.addConfigChangeListener(metricTarget);
    }
    


    /**
     * Returns the current metric configuration.
     *
     * @return The MetricConfiguration instance
     */
    public MetricConfiguration getMetricConfig() {
        return metricConfig;
    }

    /**
     * Returns the name of the AWS IoT thing associated with this component.
     *
     * @return The thing name
     */
    public String getThingName() {
        return thingName;
    }

    /**
     * Returns the name of this Greengrass component.
     *
     * @return The component name
     */
    public String getComponentName() {
        return componentName;
    }
    


    /**
     * Defines a new metric with its configuration and dimensions.
     *
     * @param metric The metric definition to register
     */
    public void defineMetric(Metric metric) {
        this.metrics.put(metric.getName(), metric);
    }

    /**
     * Returns whether a metric with the given name has been defined.
     *
     * @param name The metric name
     * @return true if a metric with this name has been defined, false otherwise
     */
    public boolean isMetricDefined(String name) {
        return metrics.containsKey(name);
    }

    /**
     * Flushes any metrics buffered by the underlying target (e.g. the CloudWatch batch buffer).
     */
    public void flushMetrics() {
        if (metricTarget != null) {
            metricTarget.flush();
        }
    }

    /**
     * Releases resources held by the underlying metric target (timers, clients, appenders).
     */
    public void close() {
        if (metricTarget != null) {
            metricTarget.close();
        }
    }

    /**
     * Emits metric values for a defined metric. The values will be buffered according to
     * the metric's configuration before being published.
     *
     * @param name The name of the metric to emit
     * @param measureValues Map of measure names to their values
     */
    public void emitMetric(String name, Map<String, Float> measureValues) {
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
    public void emitMetricNow(String name, Map<String, Float> measureValues) {
        if (metrics.containsKey(name)) {
            metricTarget.emitMetricNow(metrics.get(name), measureValues);
        } else {
            LOGGER.warn("Metric {} is not defined. Ignoring.", name);
        }
    }
    


}
