/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.utils;

import com.google.gson.Gson;
import com.google.gson.JsonObject;
import com.google.gson.JsonSyntaxException;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;


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
        return new Gson().toJson(jsonObject);
    }

    public static JsonObject destringify(String jsonString) {
        try {
            return new Gson().fromJson(jsonString, JsonObject.class);
        }catch(JsonSyntaxException e)  {
            LOGGER.error("Unable to deserialize string into json object: {}", jsonString);

        }
        return null;
    }


}
