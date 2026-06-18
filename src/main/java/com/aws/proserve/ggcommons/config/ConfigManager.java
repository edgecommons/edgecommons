/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.config;

import com.aws.proserve.ggcommons.ParsedCommandLine;
import com.aws.proserve.ggcommons.config.provider.ConfigProvider;
import com.aws.proserve.ggcommons.config.provider.ConfigProviderBuilder;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.apache.logging.log4j.Level;
import org.apache.logging.log4j.core.LoggerContext;
import org.apache.logging.log4j.core.appender.ConsoleAppender;
import org.apache.logging.log4j.core.config.Configuration;
import org.apache.logging.log4j.core.config.Configurator;
import org.apache.logging.log4j.core.config.builder.api.*;
import org.apache.logging.log4j.core.config.builder.impl.BuiltConfiguration;

import java.util.Collection;
import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.CopyOnWriteArrayList;

import static org.apache.logging.log4j.core.config.builder.api.ConfigurationBuilderFactory.newConfigurationBuilder;


/**
 * Manages configuration for Greengrass components including global settings, instance-specific configurations,
 * logging, metrics, heartbeat, and tag configurations. This class provides methods to access and modify
 * component configurations and handles configuration change notifications.
 */
public class ConfigManager
{
    private static final Logger LOGGER = LogManager.getLogger(ConfigManager.class);

    ConfigProvider configProvider;
    protected final String componentName;
    protected final String componentFullName;
    protected final String thingName;
    protected final CopyOnWriteArrayList<ConfigurationChangeListener> configChangeListeners = new CopyOnWriteArrayList<>();
    private boolean initializing = true;
    protected final JsonObject fullConfig;
    protected TagConfiguration tagConfig;
    protected HeartbeatConfiguration heartbeatConfig;
    protected MetricConfiguration metricConfig;
    protected JsonObject componentConfig;
    protected JsonObject globalConfig;
    protected LoggingConfiguration loggingConfig;
    protected HashMap<String, JsonObject> instanceConfigs;


    /**
     * Package-private constructor used by ConfigManagerFactory.
     * Use ConfigManagerFactory.create() instead of calling this directly.
     */
    /**
     * Protected no-arg constructor for testing/subclassing (e.g. mock configuration services).
     * Leaves the final identity fields null; subclasses are expected to override the accessors.
     */
    protected ConfigManager() {
        this.componentFullName = null;
        this.componentName = null;
        this.thingName = null;
        this.fullConfig = null;
    }

    ConfigManager(String componentFullName, String componentName, String thingName,
                 ConfigProvider configProvider, JsonObject fullConfig) {
        this.componentFullName = componentFullName;
        this.componentName = componentName;
        this.thingName = thingName;
        this.configProvider = configProvider;
        this.fullConfig = fullConfig;
        
        applyConfig(fullConfig);
        
        // Register logging configuration change listener
        addConfigChangeListener(new LoggingConfigChangeListener(this));
        
        // Note: initializing flag will be set to false by GGCommons after all initialization is complete
    }

    /**
     * Applies a new configuration to the component and notifies all registered listeners.
     *
     * @param config The new configuration to apply as a JsonObject
     */
    public void applyConfig(JsonObject config)
    {
        tagConfig = ConfigurationFactory.createTagConfiguration(config);
        loggingConfig = ConfigurationFactory.createLoggingConfiguration(config);
        heartbeatConfig = ConfigurationFactory.createHeartbeatConfiguration(config);
        metricConfig = ConfigurationFactory.createMetricConfiguration(config);
        reconfigureLogging();

        componentConfig = config.get("component").getAsJsonObject();
        globalConfig = componentConfig.has("global")
                ? componentConfig.get("global").getAsJsonObject()
                : new JsonObject();
        genInstancesMap();
        if (!initializing) {
            notifyConfigurationChanged();
        }
    }


    /**
     * Generates a map of instance configurations from the full configuration.
     * This is an internal method used to organize instance-specific settings.
     */
    private void genInstancesMap()
    {
        instanceConfigs = new HashMap<>();
        JsonArray instances = componentConfig.has("instances")
                ? componentConfig.get("instances").getAsJsonArray()
                : null;
        if (instances != null)
        {
            for (JsonElement instance : instances)
            {
                JsonObject instanceConfig = instance.getAsJsonObject();
                instanceConfigs.put(instanceConfig.get("id").getAsString(), instanceConfig);
                LOGGER.debug("Loaded instance config for {}", instanceConfig.get("id"));
            }
        }
    }


    /**
     * Returns the global configuration that applies to all instances.
     *
     * @return JsonObject containing global configuration settings
     */
    public JsonObject getGlobalConfig()
    {
        return globalConfig;
    }

    /**
     * Returns the collection of all instance IDs defined in the configuration.
     *
     * @return Collection of instance identifier strings
     */
    public Collection<String> getInstanceIds()
    {
        return instanceConfigs.keySet();
    }

    /**
     * Returns the configuration for a specific instance.
     *
     * @param instanceId The identifier of the instance
     * @return JsonObject containing instance-specific configuration
     */
    public JsonObject getInstanceConfig(String instanceId)
    {
        return instanceConfigs.getOrDefault(instanceId, null);
    }

    /**
     * Returns the complete configuration including global and instance-specific settings.
     *
     * @return JsonObject containing the full configuration
     */
    public JsonObject getFullConfig() { return fullConfig; }

    /**
     * Returns the tag configuration settings.
     *
     * @return TagConfiguration object containing tag-related settings
     */
    public TagConfiguration getTagConfig()
    {
        return tagConfig;
    }

    /**
     * Returns the heartbeat configuration settings.
     *
     * @return HeartbeatConfiguration object containing heartbeat-related settings
     */
    public HeartbeatConfiguration getHeartbeatConfig()
    {
        return heartbeatConfig;
    }

    /**
     * Returns the logging configuration settings.
     *
     * @return LoggingConfiguration object containing logging-related settings
     */
    public LoggingConfiguration getLoggingConfig()
    {
        return loggingConfig;
    }

    /**
     * Returns the metric configuration settings.
     *
     * @return MetricConfiguration object containing metric-related settings
     */
    public MetricConfiguration getMetricConfig() {
        return metricConfig;
    }

    /**
     * Returns the name of the AWS IoT thing associated with this component.
     *
     * @return The thing name or null if not available
     */
    public String getThingName()
    {
        return thingName;
    }

    /**
     * Returns the short name of this component.
     *
     * @return The component name
     */
    public String getComponentName()
    {
        return componentName;
    }

    /**
     * Returns the full qualified name of this component.
     *
     * @return The fully qualified component name
     */
    public String getComponentFullName()
    {
        return componentFullName;
    }

    /**
     * Adds a listener to be notified of configuration changes.
     *
     * @param listener The listener to add
     */
    public void addConfigChangeListener(ConfigurationChangeListener listener)
    {
        configChangeListeners.add(listener);
    }

    /**
     * Removes a previously added configuration change listener.
     *
     * @param listener The listener to remove
     */
    public void removeConfigChangeListener(ConfigurationChangeListener listener)
    {
        configChangeListeners.remove(listener);
    }

    /**
     * Notifies all registered configuration change listeners of a configuration change.
     * This should only be called for actual runtime configuration changes, not during initialization.
     */
    public void notifyConfigurationChanged()
    {
        LOGGER.info("configurationChanged: Notifying {} listeners", configChangeListeners.size());
        for (ConfigurationChangeListener listener : configChangeListeners)
        {
            if (listener == null) {
                LOGGER.error("ConfigurationChangeListener is null.  Not notifying.");
                continue;
            }
            // Isolate each listener: one listener throwing must not prevent the others from being notified.
            try {
                listener.onConfigurationChanged();
            } catch (Exception e) {
                LOGGER.error("ConfigurationChangeListener {} threw during notification: {}",
                        listener.getClass().getName(), e.getMessage(), e);
            }
        }
    }
    
    /**
     * Marks initialization as complete. Called by GGCommons after all initialization is finished.
     * Future configuration changes will trigger listener notifications.
     */
    public void completeInitialization()
    {
        initializing = false;
        LOGGER.debug("ConfigManager initialization completed - listeners will now be notified of configuration changes");
    }


    /**
     * Resolves a template string by replacing placeholders with actual values.
     * Supports component name, thing name, and other configuration-based substitutions.
     *
     * @param template The template string containing placeholders
     * @return The resolved string with substituted values
     */
    public String resolveTemplate(String template) {
        String retVal = template;
        if (template.contains("{ThingName}"))
        {
            retVal = retVal.replace("{ThingName}", getThingName());
        }
        if (template.contains("{ComponentName}"))
        {
            retVal = retVal.replace("{ComponentName}", getComponentName());
        }
        if (template.contains("{ComponentFullName}"))
        {
            retVal = retVal.replace("{ComponentFullName}", getComponentFullName());
        }

        if (null != tagConfig && tagConfig.getKeys() != null)
        {
            for (String tagKey : tagConfig.getKeys())
            {
                String hierarchyLevelTemplate = "{" + tagKey + "}";
                if (retVal.contains(hierarchyLevelTemplate))
                {
                    retVal = retVal.replace(hierarchyLevelTemplate, tagConfig.getKeyValue(tagKey));
                }
            }
        }

        return retVal;
    }

    /**
     * Reconfigures the logging system based on the current logging configuration.
     * Can operate in global mode (controls entire app) or isolated mode (GGCommons only).
     */
    public void reconfigureLogging()
    {
        // Check if global logging control is enabled
        boolean globalControl = getLoggingConfig().toDict().has("globalControl") && 
                               getLoggingConfig().toDict().get("globalControl").getAsBoolean();
        
        if (globalControl) {
            new GlobalLoggingManager(this, true).configureGlobalLogging();
            return;
        }
        
        try {
            // Keep the old implementation commented out for reference
//        ConfigurationBuilder<BuiltConfiguration> configBuilder = newConfigurationBuilder();
//
//        AppenderComponentBuilder consoleAppenderBuilder = configBuilder.newAppender("stdout", "Console");
//        configBuilder.add(consoleAppenderBuilder);
//
//
//        LayoutComponentBuilder layoutComponentBuilder = configBuilder.newLayout("PatternLayout");
//        layoutComponentBuilder.addAttribute("pattern", getLoggingConfig().getFormat());
//
//        consoleAppenderBuilder.addComponent(layoutComponentBuilder);
//
//        configBuilder.add(consoleAppenderBuilder);
//
//        RootLoggerComponentBuilder rootLogger = configBuilder.newRootLogger(getLoggingConfig().getLevel());
//        rootLogger.add(configBuilder.newAppenderRef("stdout"));
//        configBuilder.add(rootLogger);
//
//        Configurator.reconfigure(configBuilder.build());
//        Configurator.setAllLevels(LogManager.getRootLogger().getName(), getLoggingConfig().getLevel());

            // Get the current logger context
            LoggerContext context = (LoggerContext) LogManager.getContext(false);
            
            // Create a new configuration builder
            ConfigurationBuilder<BuiltConfiguration> builder = newConfigurationBuilder();
            
            // Set basic configuration properties
            builder.setStatusLevel(Level.INFO);
            builder.setConfigurationName("DynamicConfig-" + componentName);
            
            // Create the console appender with the configured pattern
            LayoutComponentBuilder layoutBuilder = builder.newLayout("PatternLayout")
                .addAttribute("pattern", getLoggingConfig().getFormat());
            
            AppenderComponentBuilder consoleAppender = builder.newAppender("Console", "Console")
                .addAttribute("target", ConsoleAppender.Target.SYSTEM_OUT)
                .add(layoutBuilder);
            
            builder.add(consoleAppender);
            
            // Create a file appender if file logging is enabled
            if (getLoggingConfig().isFileLoggingEnabled() && getLoggingConfig().getLogFilePath() != null) {
                String logFilePath = getLoggingConfig().getLogFilePath();
                
                // Resolve any template variables in the file path
                logFilePath = resolveTemplate(logFilePath);
                
                AppenderComponentBuilder fileAppender = builder.newAppender("File", "File")
                    .addAttribute("fileName", logFilePath)
                    .addAttribute("append", true)
                    .add(layoutBuilder);
                
                builder.add(fileAppender);
            }
            
            // Configure the root logger with the specified level
            Level rootLevel = getLoggingConfig().getLevel();
            RootLoggerComponentBuilder rootLogger = builder.newRootLogger(rootLevel);
            rootLogger.add(builder.newAppenderRef("Console"));
            
            // Add file appender reference to root logger if file logging is enabled
            if (getLoggingConfig().isFileLoggingEnabled() && getLoggingConfig().getLogFilePath() != null) {
                rootLogger.add(builder.newAppenderRef("File"));
            }
            
            builder.add(rootLogger);
            
            // Configure specific loggers if defined
            Map<String, Level> loggerLevels = getLoggingConfig().getLoggerLevels();
            for (Map.Entry<String, Level> entry : loggerLevels.entrySet()) {
                String loggerName = entry.getKey();
                Level level = entry.getValue();
                
                LoggerComponentBuilder loggerBuilder = builder.newLogger(loggerName, level)
                    .add(builder.newAppenderRef("Console"));
                
                // Add file appender reference if file logging is enabled
                if (getLoggingConfig().isFileLoggingEnabled() && getLoggingConfig().getLogFilePath() != null) {
                    loggerBuilder.add(builder.newAppenderRef("File"));
                }
                
                // Set additivity to false to prevent duplicate logging
                loggerBuilder.addAttribute("additivity", false);
                
                builder.add(loggerBuilder);
            }
            
            // Build the new configuration
            Configuration newConfig = builder.build();
            
            // Apply the new configuration
            context.start(newConfig);
            context.updateLoggers();
            
            LOGGER.info("Logging reconfigured with root level: {} and format: {}", 
                      rootLevel, 
                      getLoggingConfig().getFormat());
            
            // Log information about configured loggers
            if (!loggerLevels.isEmpty()) {
                LOGGER.info("Configured {} specific logger levels", loggerLevels.size());
                for (Map.Entry<String, Level> entry : loggerLevels.entrySet()) {
                    LOGGER.debug("Logger '{}' configured with level: {}", entry.getKey(), entry.getValue());
                }
            }
            
            // Log information about file logging
            if (getLoggingConfig().isFileLoggingEnabled() && getLoggingConfig().getLogFilePath() != null) {
                LOGGER.info("File logging enabled with path: {}", resolveTemplate(getLoggingConfig().getLogFilePath()));
            }
            
        } catch (Exception e) {
            // If reconfiguration fails, log the error but don't crash the application
            LOGGER.error("Failed to reconfigure logging: {}", e.getMessage(), e);
            LOGGER.warn("Continuing with previous logging configuration");
        }
    }
}
