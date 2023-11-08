package com.aws.proserve.ggcommons.config;

import com.github.cliftonlabs.json_simple.JsonObject;
import com.github.cliftonlabs.json_simple.Jsoner;

import java.util.Set;

public class SourceConfiguration
{
    JsonObject hierarchy = new JsonObject();

    public SourceConfiguration(JsonObject jsonConfig)
    {
        if (jsonConfig != null)
        {
            hierarchy = jsonConfig;
        }
    }

    public JsonObject toDict()
    {
        return hierarchy;
    }

    @Override
    public String toString()
    {
        return Jsoner.serialize(toDict());
    }

    public Set<String> getKeys() {
        return hierarchy.keySet();
    }

    public String getKeyValue(String key) {
        return (String) hierarchy.get(key);
    }
}
