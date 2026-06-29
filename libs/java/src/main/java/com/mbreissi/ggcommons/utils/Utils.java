/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.utils;

import com.google.gson.Gson;
import com.google.gson.JsonObject;
import com.google.gson.JsonSyntaxException;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;


/**
 * Utility class providing common helper methods for Greengrass components.
 * This class includes methods for thread operations and JSON manipulation.
 */
public class Utils
{
    protected static final Logger LOGGER = LogManager.getLogger(Utils.class);

    /**
     * Suspends the current thread execution for the specified duration.
     * This method handles InterruptedException internally by logging a warning.
     *
     * @param millis The length of time to sleep in milliseconds
     */
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

    /**
     * Converts a JsonObject to its string representation.
     *
     * @param jsonObject The JsonObject to convert
     * @return String representation of the JSON object
     */
    public static String stringify(JsonObject jsonObject)
    {
        return new Gson().toJson(jsonObject);
    }

    /**
     * Converts a JSON string back into a JsonObject.
     *
     * @param jsonString The JSON string to parse
     * @return Parsed JsonObject
     */
    public static JsonObject destringify(String jsonString) {
        try {
            return new Gson().fromJson(jsonString, JsonObject.class);
        }catch(JsonSyntaxException e)  {
            LOGGER.error("Unable to deserialize string into json object: {}", jsonString);

        }
        return null;
    }


}
