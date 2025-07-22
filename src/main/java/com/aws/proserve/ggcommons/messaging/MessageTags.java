/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging;

import com.aws.proserve.ggcommons.config.TagConfiguration;
import com.aws.proserve.ggcommons.config.ConfigManager;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonPrimitive;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Map;

/**
 * Manages tags associated with Greengrass messages.
 * Provides functionality for tag storage, inheritance, and manipulation.
 */
public class MessageTags
{
    protected static final Logger LOGGER = LogManager.getLogger(MessageTags.class);

    String thingName;

    JsonObject tags;

    /**
     * Creates a new message tags instance for the specified thing.
     *
     * @param thingName The name of the AWS IoT thing
     */
    public MessageTags(String thingName)
    {
        this.thingName = thingName;
        tags = new JsonObject();
    }

    public MessageTags(String thingName, JsonObject tags)
    {
        this.thingName = thingName;
        this.tags = tags;
    }

    /**
     * Creates a MessageTags instance from configuration settings.
     *
     * @param configManager The configuration manager containing tag settings
     * @return A new MessageTags instance with configured tags
     */
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

    public void injectTag(String key, String value)
    {
        tags.addProperty(key, value);
    }

    public static MessageTags fromDict(JsonObject src)
    {
        String thing = src.has("thing") ? src.get("thing").getAsString() : null;
        JsonObject tagsDict = new JsonObject();
        for (Map.Entry<String, JsonElement> entry : src.entrySet())
        {
            if (!entry.getKey().equals("thing"))
                tagsDict.add(entry.getKey(), entry.getValue());
        }
        return new MessageTags(thing, tagsDict);
    }

    public Map<String, JsonElement> toDict()
    {
        final Map<String, JsonElement> retVal = tags.asMap();
        retVal.put("thing", new JsonPrimitive(thingName));
        return retVal;
    }

    @Override
    public String toString()
    {
        return toDict().toString();
    }
}
