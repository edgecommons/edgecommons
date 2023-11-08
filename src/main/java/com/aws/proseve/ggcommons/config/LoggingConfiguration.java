package com.aws.proseve.ggcommons.config;

import com.github.cliftonlabs.json_simple.JsonObject;
import com.github.cliftonlabs.json_simple.Jsoner;
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
            if (jsonConfig.containsKey("level"))
                level = (String) jsonConfig.get("level");
            if (jsonConfig.containsKey("format"))
                format = (String) jsonConfig.get("format");
        }
    }

    public JsonObject toDict()
    {
        JsonObject retVal = new JsonObject();
        retVal.put("level", level);
        retVal.put("format", format);
        return retVal;
    }

    @Override
    public String toString()
    {
        return Jsoner.serialize(toDict());
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
