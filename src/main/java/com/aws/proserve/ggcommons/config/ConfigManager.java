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
import org.apache.logging.log4j.core.config.Configurator;
import org.apache.logging.log4j.core.config.builder.api.*;
import org.apache.logging.log4j.core.config.builder.impl.BuiltConfiguration;

import java.util.ArrayList;
import java.util.Collection;
import java.util.HashMap;

import static org.apache.logging.log4j.core.config.builder.api.ConfigurationBuilderFactory.newConfigurationBuilder;


public class ConfigManager
{
    private static final Logger LOGGER = LogManager.getLogger(ConfigManager.class);

    ConfigProvider configProvider;
    protected final String componentName;
    protected final String componentFullName;
    protected final String thingName;
    protected final ArrayList<ConfigurationChangeListener> configChangeListeners = new ArrayList<>();
    protected final JsonObject fullConfig;
    protected TagConfiguration tagConfig;
    protected HeartbeatConfiguration heartbeatConfig;
    protected MetricConfiguration metricConfig;
    protected JsonObject componentConfig;
    protected JsonObject globalConfig;
    protected LoggingConfiguration loggingConfig;
    protected HashMap<String, JsonObject> instanceConfigs;


   public ConfigManager(String componentName, ParsedCommandLine cmdLine)
    {
        String[] configArgs = cmdLine.configArgs;
        this.componentFullName = componentName;
        if (componentName.contains(".")) {
            this.componentName = componentName.substring(componentName.lastIndexOf(".") + 1);
        }
        else {
            this.componentName = componentName;
        }

        if (cmdLine.thingName != null) {
            thingName = cmdLine.thingName;
        }
        else if (System.getenv("AWS_IOT_THING_NAME") != null) {
            thingName = System.getenv("AWS_IOT_THING_NAME");
        }
        else {
            thingName = "NOT_GREENGRASS";
        }
        configProvider = ConfigProviderBuilder.build(this, componentName, thingName, configArgs);

        fullConfig = configProvider.loadConfiguration();
        if (fullConfig != null)
        {
            applyConfig(fullConfig);
            LOGGER.info("Configuration loaded from {}", configProvider.getConfigSource());
        }  else {
            LOGGER.error("No configuration found.  Exiting.");
            System.exit(1);
        }
    }

    public void applyConfig(JsonObject config)
    {
        tagConfig = config.has("tags")
                ? new TagConfiguration(config.get("tags").getAsJsonObject())
                : new TagConfiguration(null);
        loggingConfig = config.has("logging")
                ? new LoggingConfiguration(config.get("logging").getAsJsonObject())
                : new LoggingConfiguration(null);
        heartbeatConfig = config.has("heartbeat")
                ? new HeartbeatConfiguration((JsonObject) config.get("heartbeat"))
                : new HeartbeatConfiguration(null);
        metricConfig = config.has("metricEmission")
                ? new MetricConfiguration((JsonObject) config.get("metricEmission"))
                : new MetricConfiguration(null);
        reconfigureLogging();

        componentConfig = config.get("component").getAsJsonObject();
        globalConfig = componentConfig.has("global")
                ? componentConfig.get("global").getAsJsonObject()
                : new JsonObject();
        genInstancesMap();
        LOGGER.info("configurationChanged: Notifying {} listeners", configChangeListeners.size());
        for (ConfigurationChangeListener listener : configChangeListeners)
        {
            if (listener != null) {
                listener.onConfigurationChanged();
            } else {
                LOGGER.error("ConfigurationChangeListener is null.  Not notifying.");
            }
        }
    }


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


    public JsonObject getGlobalConfig()
    {
        return globalConfig;
    }

    public Collection<String> getInstanceIds()
    {
        return instanceConfigs.keySet();
    }

    public JsonObject getInstanceConfig(String instanceId)
    {
        return instanceConfigs.getOrDefault(instanceId, null);
    }

    public JsonObject getFullConfig() { return fullConfig; }

    public TagConfiguration getTagConfig()
    {
        return tagConfig;
    }

    public HeartbeatConfiguration getHeartbeatConfig()
    {
        return heartbeatConfig;
    }

    public LoggingConfiguration getLoggingConfig()
    {
        return loggingConfig;
    }

    public MetricConfiguration getMetricConfig() {
        return metricConfig;
    }

    public String getThingName()
    {
        return thingName;
    }

    public String getComponentName()
    {
        return componentName;
    }

    public String getComponentFullName()
    {
        return componentFullName;
    }

    public void addConfigChangeListener(ConfigurationChangeListener listener)
    {
        configChangeListeners.add(listener);
    }

    public void removeConfigChangeListener(ConfigurationChangeListener listener)
    {
        configChangeListeners.remove(listener);
    }


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

    public void reconfigureLogging()
    {
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

        LOGGER.warn("Logging reconfiguration not supported in ggcommons Java version");
    }
}
