package com.aws.proserve.ggcommons.config.manager;

import com.github.cliftonlabs.json_simple.JsonArray;
import com.github.cliftonlabs.json_simple.JsonObject;
import com.aws.proserve.ggcommons.config.HeartbeatConfiguration;
import com.aws.proserve.ggcommons.config.LoggingConfiguration;
import com.aws.proserve.ggcommons.config.TagConfiguration;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.ArrayList;
import java.util.Collection;
import java.util.HashMap;


public abstract class ConfigManager
{
    protected static final Logger LOGGER = LogManager.getLogger(ConfigManager.class);

    protected final String componentName;
    protected final String thingName;
    protected final ArrayList<ConfigurationChangeListener> configChangeListeners = new ArrayList<>();
    protected TagConfiguration tagConfig;
    protected HeartbeatConfiguration heartbeatConfig;
    protected JsonObject componentConfig;
    protected JsonObject globalConfig;
    protected LoggingConfiguration loggingConfig;
    protected final HashMap<String, JsonObject> instanceConfigs = new HashMap<>();


    ConfigManager(String componentName)
    {
        this.componentName = componentName;
        thingName = System.getenv("AWS_IOT_THING_NAME") != null ? System.getenv("AWS_IOT_THING_NAME") : "NOT_GREENGRASS";
    }

    protected void init()
    {
        JsonObject config = loadConfiguration();
        if (config != null)
        {
            applyConfig(config);
        }
    }

    private void applyConfig(JsonObject config)
    {
        loggingConfig = config.containsKey("logging") ? new LoggingConfiguration((JsonObject) config.get("logging")) : null;
        tagConfig = config.containsKey("tags") ? new TagConfiguration((JsonObject) config.get("tags")) : null;
        heartbeatConfig = config.containsKey("heartbeat") ? new HeartbeatConfiguration((JsonObject) config.get("heartbeat")) : null;

        componentConfig = (JsonObject) config.get("component");
        globalConfig = componentConfig.containsKey("global") ? (JsonObject) componentConfig.get("global") : null;
        genInstancesMap();
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

    abstract protected JsonObject loadConfiguration();

    abstract protected String getConfigSource();

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

    protected void configurationChanged(JsonObject newConfig)
    {
        LOGGER.info("configurationChanged: Applying new com.aws.proseve.ggcommons.config: {}", newConfig);
        applyConfig(newConfig);

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

        for (String tagKey : tagConfig.getKeys())
        {
            String hierarchyLevelTemplate = "{" + tagKey + "}";
            if (retVal.contains(hierarchyLevelTemplate))
            {
                retVal = retVal.replace(hierarchyLevelTemplate, tagConfig.getKeyValue(tagKey));
            }
        }
        return retVal;
    }

    protected JsonObject getDefaultConfig()
    {
        JsonObject retVal = new JsonObject();
        JsonObject logging = new JsonObject();
        JsonObject heartbeat = new JsonObject();
        JsonObject source = new JsonObject();
        JsonObject component = new JsonObject();
        JsonObject global = new JsonObject();
        JsonArray instances = new JsonArray();

        component.put("global", global);
        component.put("instances", instances);
        retVal.put("logging", logging);
        retVal.put("source", source);
        retVal.put("heartbeat", heartbeat);
        retVal.put("component", component);
        return retVal;
    }
}


