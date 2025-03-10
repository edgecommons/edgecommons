/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.google.gson.Gson;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonSyntaxException;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Map;


public class Message
{
    protected static final Logger LOGGER = LogManager.getLogger(Message.class);

    MessageHeader header;
    MessageTags tags;
    Object body;
    Object raw;

    private Message()
    {
        header = null;
        tags = null;
        body = null;
        raw = null;
    }

    public JsonObject toDict()
    {
        JsonObject retVal = new JsonObject();
        if (raw == null)
        {
            if (header != null)
                retVal.add("header", header.toDict());
            if (tags != null)
                retVal.add("tags", new Gson().toJsonTree(tags.toDict()).getAsJsonObject());
            retVal.add("body", (JsonElement) body);
        }
        else
        {
            retVal.add("raw", (JsonElement) raw);
        }
        return retVal;
    }

    @Override
    public String toString()
    {
        return toDict().toString();
    }

    public String getCorrelationId()
    {
        if (header == null)
            return null;
        return header.getCorrelationId();
    }

    public MessageHeader getHeader()
    {
        return header;
    }

    public MessageTags getTags()
    {
        return tags;
    }

    public void injectTag(String key, String value)
    {
        if (tags == null)
            tags = new MessageTags(null);
        tags.injectTag(key, value);
    }

    public Object getBody()
    {
        return body;
    }

    public Object getRaw()
    {
        return raw;
    }

    public String makeRequest()
    {
        return makeRequest(null);
    }

    public String makeRequest(String replyTo)
    {
        if (header == null)
        {
            header = new MessageHeader("None", "None", null);
            LOGGER.warn("Attempting to make request from message with no header");
        }
        return header.makeRequest(replyTo);
    }

    public void setCorrelationId(String correlationId)
    {
        if (header == null)
            header = new MessageHeader("None", "None", correlationId);
        else
            header.setCorrelationId(correlationId);
    }

    public static Message buildFromConfig(String name, String version, Object payload,
                                          ConfigManager configManager)
    {
        return Message.buildFromConfig(name, version, payload, configManager, null);
    }

    public static Message buildFromConfig(String name, String version, Object payload,
                                          ConfigManager configManager, String correlationId)
    {
        Message retVal = new Message();
        retVal.header = new MessageHeader(name, version, correlationId);
        retVal.tags = MessageTags.fromConfig(configManager);
        if (payload instanceof String)
        {
            String payloadStr =(String) payload;
            try
            {
                Gson gson = new Gson();
                // check if a "stringified" json object and convert to object if so
                retVal.body = gson.fromJson(payloadStr, Object.class);
            }
            catch (JsonSyntaxException e)
            {
                // just a regular string
                retVal.body = payloadStr;
            }
        }
        else
        {
            retVal.body = payload;
        }
        return retVal;
    }

    public static Message build(Object msgContents)
    {
        Message retVal = new Message();
        LOGGER.trace("In Message::build");
        if (msgContents instanceof JsonObject)
        {
            JsonObject msgJsonObj = (JsonObject) msgContents;
            LOGGER.trace("Message contents: {}", msgJsonObj);
            if (msgJsonObj.has("header"))
            {
                LOGGER.trace("processing header");
                retVal.header = MessageHeader.fromDict(msgJsonObj.getAsJsonObject("header"));
                LOGGER.trace("header deserialized");
            }
            if (msgJsonObj.has("tags"))
            {
                LOGGER.trace("processing tags");
                retVal.tags = MessageTags.fromDict(msgJsonObj.getAsJsonObject("tags"));
                LOGGER.trace("source deserialized");
            }
            if (msgJsonObj.has("body"))
            {
                LOGGER.trace("processing body");
                retVal.body =  msgJsonObj.getAsJsonObject("body");
                LOGGER.trace("body desiralized");
            }
            if (!(msgJsonObj.has("header") || msgJsonObj.has("tags") || msgJsonObj.has("body")))
            {
                LOGGER.trace("Json contained raw string: Assigning to raw");
                retVal.raw = msgJsonObj;
            }
        }
        else
        {
            LOGGER.trace("Message not instance of JsonObject, assigning to raw");
            retVal.raw = msgContents;
        }
        return retVal;
    }

}
