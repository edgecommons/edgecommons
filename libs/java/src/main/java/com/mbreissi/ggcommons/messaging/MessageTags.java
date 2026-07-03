/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.messaging;

import com.mbreissi.ggcommons.config.TagConfiguration;
import com.mbreissi.ggcommons.config.ConfigManager;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Map;

/**
 * Manages the business-context tags associated with Greengrass messages.
 * Provides functionality for tag storage, inheritance, and manipulation.
 *
 * <p>UNS hard cut: the synthesized {@code thing} tag is gone — the device now travels in the
 * top-level {@code identity} envelope element (its last {@code hier} entry). A stray inbound
 * {@code thing} key is treated as an ordinary tag (no special-casing, no legacy shim).
 */
public class MessageTags
{
    protected static final Logger LOGGER = LogManager.getLogger(MessageTags.class);

    JsonObject tags;

    /**
     * Creates a new, empty message tags instance.
     */
    public MessageTags()
    {
        tags = new JsonObject();
    }

    /**
     * Creates a message tags instance wrapping the supplied tag object.
     *
     * @param tags The backing tag object
     */
    public MessageTags(JsonObject tags)
    {
        this.tags = tags;
    }

    /**
     * Creates a MessageTags instance from configuration settings.
     *
     * @param configService The configuration manager containing tag settings
     * @return A new MessageTags instance with configured tags
     */
    public static MessageTags fromConfig(ConfigManager configService)
    {
        TagConfiguration sourceConfig = configService.getTagConfig();
        if (sourceConfig != null)
        {
            return new MessageTags(sourceConfig.toDict());
        }
        else
        {
            return new MessageTags(new JsonObject());
        }
    }

    public void injectTag(String key, String value)
    {
        tags.addProperty(key, value);
    }

    public static MessageTags fromDict(JsonObject src)
    {
        JsonObject tagsDict = new JsonObject();
        for (Map.Entry<String, JsonElement> entry : src.entrySet())
        {
            tagsDict.add(entry.getKey(), entry.getValue());
        }
        return new MessageTags(tagsDict);
    }

    public Map<String, JsonElement> toDict()
    {
        return new java.util.LinkedHashMap<>(tags.asMap());
    }

    @Override
    public String toString()
    {
        return toDict().toString();
    }
}
