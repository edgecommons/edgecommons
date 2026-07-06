/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

import com.mbreissi.edgecommons.platform.PlatformResolver;
import org.apache.logging.log4j.Level;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.core.LoggerContext;
import org.apache.logging.log4j.core.config.Configuration;
import org.apache.logging.log4j.core.config.builder.api.*;
import org.apache.logging.log4j.core.config.builder.impl.BuiltConfiguration;

import java.util.Map;

import static org.apache.logging.log4j.core.config.builder.api.ConfigurationBuilderFactory.newConfigurationBuilder;

/**
 * Manages global logging configuration for the entire application.
 * Takes full control of the logging system when enabled.
 */
public class GlobalLoggingManager {
    
    private final ConfigManager configManager;
    private final boolean takeGlobalControl;
    
    public GlobalLoggingManager(ConfigManager configManager, boolean takeGlobalControl) {
        this.configManager = configManager;
        this.takeGlobalControl = takeGlobalControl;
    }
    
    /**
     * Configures logging globally for the entire application.
     * Replaces the entire logging configuration.
     */
    public void configureGlobalLogging() {
        if (!takeGlobalControl) {
            return;
        }
        
        try {
            LoggingConfiguration loggingConfig = configManager.getLoggingConfig();
            LoggerContext context = (LoggerContext) LogManager.getContext(false);
            
            // Build complete new configuration
            ConfigurationBuilder<BuiltConfiguration> builder = newConfigurationBuilder();
            builder.setConfigurationName("EdgeCommons-Global-Config");
            
            // FR-LOG-1/4 (precedence FR-RT-3): resolve the effective logging-format token the SAME way
            // as ConfigManager.reconfigureLogging() — explicit `logging.java_format` ▸ platform-profile
            // default (`json` on KUBERNETES) ▸ library default — so the global-control path honors the
            // stdout-JSON sink and the KUBERNETES default identically (no globalControl bypass).
            String effectiveFormat = configManager.resolveEffectiveLogFormat();
            boolean jsonSink = PlatformResolver.LOGGING_FORMAT_JSON.equalsIgnoreCase(effectiveFormat);

            // FR-LOG-2: under the JSON sink (the KUBERNETES default) logging is stdout-only — no
            // in-process size-rotation, so a read-only root FS never breaks logging.
            boolean fileLogging = !jsonSink
                    && loggingConfig.isFileLoggingEnabled()
                    && loggingConfig.getLogFilePath() != null;

            // Console appender. Only the LAYOUT changes for the JSON sink: a PatternLayout built from
            // Log4j2 built-in converters that emits one valid JSON object per line (no extra dependency).
            String pattern = jsonSink
                    ? ConfigManager.buildJsonPattern(configManager.correlationFields())
                    : effectiveFormat;
            LayoutComponentBuilder layoutBuilder = builder.newLayout("PatternLayout")
                .addAttribute("pattern", pattern);
            if (jsonSink) {
                // The JSON pattern renders the exception itself (single escaped line under "thrown");
                // disable PatternLayout's automatic trailing throwable append (would break one-per-line).
                layoutBuilder.addAttribute("alwaysWriteExceptions", false);
            }

            AppenderComponentBuilder consoleAppender = builder.newAppender("Console", "Console")
                .addAttribute("target", "SYSTEM_OUT")
                .add(layoutBuilder);
            builder.add(consoleAppender);

            // File appender if enabled — size-rotated (maxFileSize / backupCount), matching the
            // Python/Rust RotatingFileHandler contract. Never installed under the JSON sink.
            if (fileLogging) {
                String logFilePath = configManager.resolveTemplate(loggingConfig.getLogFilePath());
                AppenderComponentBuilder fileAppender = builder.newAppender("File", "RollingFile")
                    .addAttribute("fileName", logFilePath)
                    .addAttribute("filePattern", logFilePath + ".%i")
                    .add(layoutBuilder)
                    .addComponent(builder.newComponent("Policies")
                        .addComponent(builder.newComponent("SizeBasedTriggeringPolicy")
                            .addAttribute("size", loggingConfig.getMaxFileSize())))
                    .addComponent(builder.newComponent("DefaultRolloverStrategy")
                        .addAttribute("max", loggingConfig.getBackupCount())
                        .addAttribute("fileIndex", "min"));
                builder.add(fileAppender);
            }

            // Root logger
            RootLoggerComponentBuilder rootLogger = builder.newRootLogger(loggingConfig.getLevel());
            rootLogger.add(builder.newAppenderRef("Console"));
            if (fileLogging) {
                rootLogger.add(builder.newAppenderRef("File"));
            }
            builder.add(rootLogger);

            // Specific loggers
            for (Map.Entry<String, Level> entry : loggingConfig.getLoggerLevels().entrySet()) {
                LoggerComponentBuilder loggerBuilder = builder.newLogger(entry.getKey(), entry.getValue())
                    .add(builder.newAppenderRef("Console"))
                    .addAttribute("additivity", false);
                if (fileLogging) {
                    loggerBuilder.add(builder.newAppenderRef("File"));
                }
                builder.add(loggerBuilder);
            }
            
            // Apply globally
            context.start(builder.build());
            context.updateLoggers();
            
        } catch (Exception e) {
            System.err.println("Failed to configure global logging: " + e.getMessage());
        }
    }
}