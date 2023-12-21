package com.aws.proserve.ggcommons.config;

import com.aws.proserve.ggcommons.config.provider.ConfigProvider;
import com.aws.proserve.ggcommons.config.provider.ConfigProviderBuilder;
import com.aws.proserve.ggcommons.config.provider.ConfigurationChangeListener;
import com.github.cliftonlabs.json_simple.JsonArray;
import com.github.cliftonlabs.json_simple.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.ArrayList;
import java.util.Collection;
import java.util.HashMap;


public class ConfigManager
{
    private static final Logger LOGGER = LogManager.getLogger(ConfigManager.class);

    ConfigProvider configProvider;
    protected final String componentName;
    protected final String thingName;
    protected final ArrayList<ConfigurationChangeListener> configChangeListeners = new ArrayList<>();
    protected TagConfiguration tagConfig;
    protected HeartbeatConfiguration heartbeatConfig;
    protected JsonObject componentConfig;
    protected JsonObject globalConfig;
    protected LoggingConfiguration loggingConfig;
    protected final HashMap<String, JsonObject> instanceConfigs = new HashMap<>();


   public ConfigManager(String componentName, String[] configArgs)
    {
        this.componentName = componentName;
        thingName = System.getenv("AWS_IOT_THING_NAME") != null ? System.getenv("AWS_IOT_THING_NAME") : "NOT_GREENGRASS";
        configProvider = ConfigProviderBuilder.build(this, componentName, thingName, configArgs);

        JsonObject config = configProvider.loadConfiguration();
        if (config != null)
        {
            applyConfig(config);
        }
    }

    public void applyConfig(JsonObject config)
    {
        loggingConfig = config.containsKey("logging") ? new LoggingConfiguration((JsonObject) config.get("logging")) : null;
        tagConfig = config.containsKey("tags") ? new TagConfiguration((JsonObject) config.get("tags")) : null;
        heartbeatConfig = config.containsKey("heartbeat") ? new HeartbeatConfiguration((JsonObject) config.get("heartbeat")) : null;

        componentConfig = (JsonObject) config.get("component");
        globalConfig = componentConfig.containsKey("global") ? (JsonObject) componentConfig.get("global") : null;
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
        JsonArray instances = componentConfig.containsKey("instances") ? (JsonArray) componentConfig.get("instances") : null;
        if (instances != null)
        {
            for (Object instance : instances)
            {
                JsonObject instanceConfig = (JsonObject) instance;
                instanceConfigs.put((String) instanceConfig.get("id"), instanceConfig);
                LOGGER.debug("Loaded com.aws.proseve.ggcommons.config for {}", instanceConfig.get("id"));
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

    public String getThingName()
    {
        return thingName;
    }

    public String getComponentName()
    {
        return componentName;
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


}
