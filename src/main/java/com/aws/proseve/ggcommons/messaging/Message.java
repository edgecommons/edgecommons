package com.aws.proseve.ggcommons.messaging;

import com.github.cliftonlabs.json_simple.JsonException;
import com.github.cliftonlabs.json_simple.JsonObject;
import com.github.cliftonlabs.json_simple.Jsoner;
import com.aws.proseve.ggcommons.config.manager.ConfigManager;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Map;


public class Message
{
    protected static final Logger LOGGER = LogManager.getLogger(Message.class);

    MessageHeader header;
    MessageSource source;
    Object body;
    Object raw;

    private Message()
    {
        header = null;
        source = null;
        body = null;
        raw = null;
    }

    public JsonObject toDict()
    {
        JsonObject retVal = new JsonObject();
        if (raw == null)
        {
            if (header != null)
                retVal.put("header", header.toDict());
            if (source != null)
                retVal.put("source", source.toDict());
            retVal.put("body", body);
        }
        else
        {
            retVal.put("raw", raw);
        }
        return retVal;
    }

    @Override
    public String toString()
    {
        return Jsoner.serialize(toDict());
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

    public MessageSource getSource()
    {
        return source;
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
            header = new MessageHeader("None", "None", "0.1");
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
        retVal.source = MessageSource.fromConfig(configManager);
        if (payload instanceof String)
        {
            try
            {
                // check if a "stringified" json object and convert to object if so
                retVal.body = Jsoner.deserialize((String) payload);
            }
            catch (JsonException e)
            {
                // just a regular string
                retVal.body = payload;
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
        if (msgContents instanceof JsonObject msgJsonObj)
        {
            LOGGER.trace("Message contents: {}", msgJsonObj.toJson());
            if (msgJsonObj.containsKey("header"))
            {
                LOGGER.trace("processing header");
                retVal.header = MessageHeader.fromDict((Map<String, Object>) msgJsonObj.get("header"));
                LOGGER.trace("header deserialized");
            }
            if (msgJsonObj.containsKey("source"))
            {
                LOGGER.trace("processing source");
                retVal.source = MessageSource.fromDict((Map<String, Object>) msgJsonObj.get("source"));
                LOGGER.trace("source deserialized");
            }
            if (msgJsonObj.containsKey("body"))
            {
                LOGGER.trace("processing body");
                retVal.body =  new JsonObject((Map<String, Object>) msgJsonObj.get("body"));
                LOGGER.trace("body desiralized");
            }
            if (!(msgJsonObj.containsKey("header") && msgJsonObj.containsKey("source") && msgJsonObj.containsKey("body")))
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
