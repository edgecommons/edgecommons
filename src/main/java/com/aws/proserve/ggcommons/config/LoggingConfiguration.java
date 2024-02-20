package com.aws.proserve.ggcommons.config;

import com.google.gson.Gson;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.Level;

public class LoggingConfiguration
{
    static String DEFAULT_LEVEL = "INFO";
    static String DEFAULT_FORMAT = "%d{yyyy-MM-dd HH:mm:ss} [%-5p] %-25.25c{1}(%4L): %m%n";

    String level = DEFAULT_LEVEL;
    String format = DEFAULT_FORMAT;

    public LoggingConfiguration(JsonObject jsonConfig)
    {
        if (jsonConfig != null)
        {
            if (jsonConfig.has("level"))
                level = String.valueOf(jsonConfig.get("level"));
            if (jsonConfig.has("format"))
                format = String.valueOf(jsonConfig.get("format"));
        }
    }

    public JsonObject toDict()
    {
        JsonObject retVal = new JsonObject();
        retVal.addProperty("level", level);
        retVal.addProperty("format", format);
        return retVal;
    }

    @Override
    public String toString()
    {
        Gson gson = new Gson();
        return gson.toJson(toDict(), JsonObject.class);
    }

    Level getLevel()
    {
        return Level.toLevel(level);
    }

    String getFormat()
    {
        return format;
    }
}
