package com.aws.proserve.ggcommons.utils;

import com.github.cliftonlabs.json_simple.JsonException;
import com.github.cliftonlabs.json_simple.JsonObject;
import com.github.cliftonlabs.json_simple.Jsoner;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.IOException;
import java.io.StringReader;
import java.io.StringWriter;

public class Utils
{
    protected static final Logger LOGGER = LogManager.getLogger(Utils.class);

    public static void sleep(long millis)
    {
        try
        {
            Thread.sleep(millis);
        }
        catch (InterruptedException e)
        {
            LOGGER.warn("Sleep interrupted - {}", e.toString());
        }
    }

    public static String stringify(JsonObject jsonObject)
    {
        String retVal = null;
        StringWriter writer = new StringWriter();
        try {
            Jsoner.prettyPrint(new StringReader(Jsoner.serialize(jsonObject)), writer, "", "");
            retVal = Jsoner.escape(writer.toString());
        } catch (JsonException | IOException e) {
            LOGGER.error("Unable to stringify json object: {}", jsonObject.toString());
        }
        return retVal;
    }

    public static JsonObject destringify(String jsonString) {
        JsonObject retVal = null;
        try {
            retVal = (JsonObject) Jsoner.deserialize(jsonString);
        } catch (JsonException e) {
            LOGGER.error("Unable to deserialize string into json object: {}", jsonString);
        }
        return retVal;
    }


}
