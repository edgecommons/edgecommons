/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.metrics.targets;

import com.breissinger.ggcommons.config.ConfigManager;
import com.breissinger.ggcommons.config.MetricConfiguration;
import com.breissinger.ggcommons.metrics.Metric;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.Level;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import java.util.Map;

public final class Log extends MetricTarget
{
    private final static Logger LOGGER = LogManager.getLogger(MetricTarget.class);
    private Logger metricLogger;
    private String currentLoggerName;
    private org.apache.logging.log4j.core.appender.RollingFileAppender currentAppender;

    public Log(ConfigManager configManager)
    {
        super(configManager);
        // Don't configure logger immediately - wait for logging system to stabilize
        metricLogger = null;
    }

    @Override
    public void emitMetric(Metric metric, Map<String, Float> measureValues)
    {
        ensureMetricLoggerConfigured();
        emitMetricNow(metric, measureValues);
    }

    @Override
    public void emitMetricNow(Metric metric, Map<String, Float> measureValues)
    {
        ensureMetricLoggerConfigured();
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
        LOGGER.info("Configuration changed. Resetting metric logger");
        // Reset the logger so it gets reconfigured on next use
        metricLogger = null;
        currentLoggerName = null;
        return true;
    }
    
    private void ensureMetricLoggerConfigured()
    {
        if (metricLogger == null) {
            metricLogger = configureMetricLogger();
        }
    }

    private Logger configureMetricLogger()
    {
        MetricConfiguration metricConfig = configManager.getMetricConfig();
        String metricFile = configManager.resolveTemplate(metricConfig.getLogFileNameTemplate());
        String uniqueAppenderName = "MetricFileAppender_" + System.currentTimeMillis();
        String uniqueLoggerName = "metric_" + System.currentTimeMillis();

        // Stop any previously-created appender to avoid leaking file handles on reconfiguration.
        if (currentAppender != null) {
            currentAppender.stop();
            currentAppender = null;
        }

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
            
            // Create timestamp-based file pattern
            String filePattern = createTimestampFilePattern(metricFile);
            
            org.apache.logging.log4j.core.appender.RollingFileAppender appender = 
                org.apache.logging.log4j.core.appender.RollingFileAppender.newBuilder()
                    .withFileName(metricFile)
                    .withFilePattern(filePattern)
                    .setName(uniqueAppenderName)
                    .setLayout(layout)
                    .withPolicy(triggeringPolicy)
                    .withStrategy(rolloverStrategy)
                    .build();
            
            appender.start();
            config.addAppender(appender);
            currentAppender = appender;
            
            // Create logger configuration for metrics
            org.apache.logging.log4j.core.config.LoggerConfig loggerConfig = 
                new org.apache.logging.log4j.core.config.LoggerConfig(
                    uniqueLoggerName,
                    Level.INFO,
                    false // additivity
                );
            loggerConfig.addAppender(appender, Level.INFO, null);
            
            config.addLogger(uniqueLoggerName, loggerConfig);
            context.updateLoggers();
            
            currentLoggerName = uniqueLoggerName;
            LOGGER.debug("Metric logger configured to write to: {} with logger: {}", metricFile, uniqueLoggerName);
            
            return LogManager.getLogger(uniqueLoggerName);
            
        } catch (Exception e) {
            LOGGER.error("Failed to configure metric logger: {}", e.getMessage(), e);
        }
        
        // Fallback - try to get existing logger or create a basic one
        currentLoggerName = "metric_fallback";
        return LogManager.getLogger(currentLoggerName);
    }
    
    private String createTimestampFilePattern(String baseFileName) {
        // Extract file extension if present
        int lastDotIndex = baseFileName.lastIndexOf('.');
        if (lastDotIndex > 0) {
            String nameWithoutExtension = baseFileName.substring(0, lastDotIndex);
            String extension = baseFileName.substring(lastDotIndex);
            return nameWithoutExtension + "-%d{yyyyMMddHHmmss}" + extension;
        } else {
            return baseFileName + "-%d{yyyyMMddHHmmss}.log";
        }
    }

    @Override
    public void close()
    {
        if (currentAppender != null)
        {
            currentAppender.stop();
            currentAppender = null;
        }
    }
}