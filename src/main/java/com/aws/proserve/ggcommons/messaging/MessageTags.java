package com.aws.proserve.ggcommons.messaging;

import com.aws.proserve.ggcommons.config.TagConfiguration;
import com.aws.proserve.ggcommons.config.ConfigManager;
import com.github.cliftonlabs.json_simple.JsonObject;
import com.github.cliftonlabs.json_simple.Jsoner;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Map;

public class MessageTags
{
    protected static final Logger LOGGER = LogManager.getLogger(MessageTags.class);

    String thingName;

    JsonObject tags;

    public MessageTags(String thingName, JsonObject tags)
    {
        this.thingName = thingName;
        this.tags = tags;
    }

    public static MessageTags fromConfig(ConfigManager configManager)
    {
        TagConfiguration sourceConfig = configManager.getTagConfig();
        if (sourceConfig != null)
        {
            return new MessageTags(configManager.getThingName(), sourceConfig.toDict());
        }
        else
        {
            return new MessageTags(configManager.getThingName(), new JsonObject());
        }
    }

    public static MessageTags fromDict(Map<String,Object> src)
    {
        String thing = src.containsKey("thing") ? (String) src.get("thing") : null;
        JsonObject tagsDict = new JsonObject();
        src.forEach((key,value) -> {
            if (!key.equals("thing"))
                tagsDict.put(key, value);
        });
        return new MessageTags(thing, tagsDict);
    }

    public JsonObject toDict()
    {
        JsonObject retVal = new JsonObject();

        retVal.put("thing", thingName);
        tags.forEach(retVal::put);

        return retVal;
    }

    @Override
    public String toString()
    {
        return Jsoner.serialize(toDict());
    }
}
