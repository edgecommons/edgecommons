package com.aws.proserve.ggcommons.config;

import com.github.cliftonlabs.json_simple.JsonObject;
import com.github.cliftonlabs.json_simple.Jsoner;

import java.util.Set;

public class TagConfiguration
{
    JsonObject tags = new JsonObject();

    public TagConfiguration(JsonObject jsonConfig)
    {
        if (jsonConfig != null)
        {
            tags = jsonConfig;
        }
    }

    public JsonObject toDict()
    {
        return tags;
    }

    @Override
    public String toString()
    {
        return Jsoner.serialize(toDict());
    }

    public Set<String> getKeys() {
        return tags.keySet();
    }

    public String getKeyValue(String key) {
        return (String) tags.get(key);
    }
}
