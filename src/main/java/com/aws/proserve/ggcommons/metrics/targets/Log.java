/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics.targets;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.config.MetricConfiguration;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.Level;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import java.util.Map;

public class Log extends MetricTarget
{
    private final static Logger LOGGER = LogManager.getLogger(MetricTarget.class);
    private Logger metricLogger;

    public Log(ConfigManager configManager)
    {
        super(configManager);
        metricLogger = configureMetricLogger();
    }

    @Override
    public void emitMetric(Metric metric, Map<String, Float> measureValues)
    {
        emitMetricNow(metric, measureValues);
    }

    @Override
    public void emitMetricNow(Metric metric, Map<String, Float> measureValues)
    {
        JsonObject metricData = EmfHelper.buildMetricData(metricConfig.getNamespace(), metric, measureValues, false);
        metricLogger.info(metricData.toString());
        if (metricConfig.getLargeFleetWorkaround())
        {
            metricData = EmfHelper.buildMetricData(metricConfig.getNamespace(), metric, measureValues, true);
            metricLogger.info(metricData.toString());
        }
        LOGGER.trace("Metric emitted for {} emitted", metric.getName());
    }

    @Override
    public boolean onConfigurationChanged()
    {
        LOGGER.info("Configuration changed. Reconfiguring metric logger");
        
        // Clean up existing metric logger configuration
        try {
            org.apache.logging.log4j.core.LoggerContext context = 
                (org.apache.logging.log4j.core.LoggerContext) LogManager.getContext(false);
            org.apache.logging.log4j.core.config.Configuration config = context.getConfiguration();
            
            // Remove existing metric logger and appender
            config.removeLogger("metric");
            org.apache.logging.log4j.core.Appender existingAppender = config.getAppender("MetricFileAppender");
            if (existingAppender != null) {
                existingAppender.stop();
                config.getAppenders().remove("MetricFileAppender");
            }
            
        } catch (Exception e) {
            LOGGER.warn("Failed to clean up existing metric logger: {}", e.getMessage());
        }
        
        metricLogger = configureMetricLogger();
        return true;
    }

    private Logger configureMetricLogger()
    {
        MetricConfiguration metricConfig = configManager.getMetricConfig();
        String metricFile = configManager.resolveTemplate(metricConfig.getLogFileNameTemplate());
        
        try {
            // Get current context and configuration
            org.apache.logging.log4j.core.LoggerContext context = 
                (org.apache.logging.log4j.core.LoggerContext) LogManager.getContext(false);
            org.apache.logging.log4j.core.config.Configuration config = context.getConfiguration();
            
            // Create rolling file appender for metrics with size-based rotation
            org.apache.logging.log4j.core.layout.PatternLayout layout = 
                org.apache.logging.log4j.core.layout.PatternLayout.newBuilder()
                    .withPattern("%d{yyyy-MM-dd HH:mm:ss.SSS} [METRIC] %m%n") // Simple pattern for metric data
                    .build();
            
            org.apache.logging.log4j.core.appender.rolling.SizeBasedTriggeringPolicy triggeringPolicy =
                org.apache.logging.log4j.core.appender.rolling.SizeBasedTriggeringPolicy.createPolicy(metricConfig.getMaxFileSize());
            
            org.apache.logging.log4j.core.appender.rolling.DefaultRolloverStrategy rolloverStrategy =
                org.apache.logging.log4j.core.appender.rolling.DefaultRolloverStrategy.newBuilder()
                    .withMax("5")
                    .build();
            
            org.apache.logging.log4j.core.appender.RollingFileAppender appender = 
                org.apache.logging.log4j.core.appender.RollingFileAppender.newBuilder()
                    .withFileName(metricFile)
                    .withFilePattern(metricFile + ".%i")
                    .setName("MetricFileAppender")
                    .setLayout(layout)
                    .withPolicy(triggeringPolicy)
                    .withStrategy(rolloverStrategy)
                    .build();
            
            appender.start();
            config.addAppender(appender);
            
            // Create logger configuration for metrics
            org.apache.logging.log4j.core.config.LoggerConfig loggerConfig = 
                new org.apache.logging.log4j.core.config.LoggerConfig(
                    "metric",
                    Level.INFO,
                    false // additivity
                );
            loggerConfig.addAppender(appender, Level.INFO, null);
            
            config.addLogger("metric", loggerConfig);
            context.updateLoggers();
            
            LOGGER.debug("Metric logger configured to write to: {}", metricFile);
            
        } catch (Exception e) {
            LOGGER.error("Failed to configure metric logger: {}", e.getMessage(), e);
        }
        
        return LogManager.getLogger("metric");
    }
}

