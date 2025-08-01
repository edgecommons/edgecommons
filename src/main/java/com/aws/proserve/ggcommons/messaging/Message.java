/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.interfaces.IConfigurationService;
import com.google.gson.Gson;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonSyntaxException;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Map;


/**
 * Represents a message in the Greengrass messaging system.
 * Contains message headers and payload data for communication between components
 * and with AWS IoT Core.
 */
public class Message
{
    protected static final Logger LOGGER = LogManager.getLogger(Message.class);

    MessageHeader header;
    MessageTags tags;
    Object body;
    Object raw;

    /**
     * Private constructor for creating empty messages.
     * Messages should be created using the build methods.
     */
    Message()
    {
        header = null;
        tags = null;
        body = null;
        raw = null;
    }

    /**
     * Converts the message to a JsonObject representation.
     *
     * @return JsonObject containing the full message data
     */
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

    /**
     * Gets the correlation ID associated with this message.
     *
     * @return The message correlation ID
     */
    public String getCorrelationId()
    {
        if (header == null)
            return null;
        return header.getCorrelationId();
    }

    /**
     * Gets the header information for this message.
     *
     * @return The MessageHeader object
     */
    public MessageHeader getHeader()
    {
        return header;
    }

    /**
     * Gets the tags associated with this message.
     *
     * @return The MessageTags object
     */
    public MessageTags getTags()
    {
        return tags;
    }

    /**
     * Adds a tag to this message.
     *
     * @param key The tag key
     * @param value The tag value
     */
    public void injectTag(String key, String value)
    {
        if (tags == null)
            tags = new MessageTags(null);
        tags.injectTag(key, value);
    }

    /**
     * Gets the message payload body.
     *
     * @return The message payload object
     */
    public Object getBody()
    {
        return body;
    }

    /**
     * Gets the raw message content.
     *
     * @return The raw message object if present, null otherwise
     */
    public Object getRaw()
    {
        return raw;
    }

    public String makeRequest()
    {
        return makeRequest(null);
    }

    /**
     * Prepares this message as a request, setting up correlation and reply information.
     *
     * @param replyTo The topic to send replies to, or null for auto-generated topic
     * @return The correlation ID for tracking the request
     */
    public String makeRequest(String replyTo)
    {
        if (header == null)
        {
            header = new MessageHeader("None", "None", null);
            LOGGER.warn("Attempting to make request from message with no header");
        }
        return header.makeRequest(replyTo);
    }

    /**
     * Sets the correlation ID for this message.
     *
     * @param correlationId The correlation ID to use
     */
    public void setCorrelationId(String correlationId)
    {
        if (header == null)
            header = new MessageHeader("None", "None", correlationId);
        else
            header.setCorrelationId(correlationId);
    }

    /**
     * @deprecated Use {@link #buildFromConfig(String, String, Object, IConfigurationService)} instead
     */
    @Deprecated
    public static Message buildFromConfig(String name, String version, Object payload,
                                          ConfigManager configManager)
    {
        return MessageBuilder.create(name, version)
            .withPayload(payload)
            .withConfig(configManager)
            .build();
    }

    /**
     * @deprecated Use {@link #buildFromConfig(String, String, Object, IConfigurationService, String)} instead
     */
    @Deprecated
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

    /**
     * @deprecated Use {@link MessageBuilder#create(String, String)} instead
     */
    @Deprecated
    public static Message buildFromConfig(String name, String version, Object payload,
                                          IConfigurationService configService)
    {
        return MessageBuilder.create(name, version)
            .withPayload(payload)
            .withConfig(configService)
            .build();
    }

    /**
     * @deprecated Use {@link MessageBuilder#create(String, String)} instead
     */
    @Deprecated
    public static Message buildFromConfig(String name, String version, Object payload,
                                          IConfigurationService configService, String correlationId)
    {
        Message retVal = new Message();
        retVal.header = new MessageHeader(name, version, correlationId);
        retVal.tags = MessageTags.fromConfig(configService);
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

    /**
     * Builds a message from a generic message contents object.
     *
     * @param msgContents The content to create the message from
     * @return A new Message instance
     * @deprecated Use {@link MessageBuilder#fromObject(Object)} instead
     */
    @Deprecated
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
