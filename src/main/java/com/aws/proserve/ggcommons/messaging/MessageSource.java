package com.aws.proserve.ggcommons.messaging;

import com.aws.proserve.ggcommons.config.SourceConfiguration;
import com.aws.proserve.ggcommons.config.manager.ConfigManager;
import com.github.cliftonlabs.json_simple.JsonObject;
import com.github.cliftonlabs.json_simple.Jsoner;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Map;

public class MessageSource
{
    protected static final Logger LOGGER = LogManager.getLogger(MessageSource.class);

    String thingName;

    JsonObject hierarchy;

    public MessageSource(String thingName, JsonObject hierarchy)
    {
        this.thingName = thingName;
        this.hierarchy = hierarchy;
    }

    public static MessageSource fromConfig(ConfigManager configManager)
    {
        SourceConfiguration sourceConfig = configManager.getSourceConfig();
        if (sourceConfig != null)
        {
            return new MessageSource(configManager.getThingName(), sourceConfig.toDict());
        }
        else
        {
            return new MessageSource(configManager.getThingName(), new JsonObject());
        }
    }

    public static MessageSource fromDict(Map<String,Object> src)
    {
        String thing = src.containsKey("thing") ? (String) src.get("thing") : null;
        JsonObject hierarchy = new JsonObject();
        src.forEach((key,value) -> {
            if (!key.equals("thing"))
                hierarchy.put(key, value);
        });
        return new MessageSource(thing, hierarchy);
    }

    public JsonObject toDict()
    {
        JsonObject retVal = new JsonObject();

        retVal.put("thing", thingName);
        hierarchy.forEach(retVal::put);

        return retVal;
    }

    @Override
    public String toString()
    {
        return Jsoner.serialize(toDict());
    }
}
