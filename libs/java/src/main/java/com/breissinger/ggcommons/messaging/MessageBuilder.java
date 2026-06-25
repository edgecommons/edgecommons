package com.breissinger.ggcommons.messaging;

import com.breissinger.ggcommons.config.ConfigManager;
import com.google.gson.Gson;
import com.google.gson.JsonObject;
import com.google.gson.JsonSyntaxException;

/**
 * Builder for creating Message instances with fluent API.
 */
public class MessageBuilder {
    private String name;
    private String version;
    private String correlationId;
    private Object payload;
    private ConfigManager configService;
    
    private MessageBuilder(String name, String version) {
        this.name = name;
        this.version = version;
    }
    
    public static MessageBuilder create(String name, String version) {
        return new MessageBuilder(name, version);
    }
    
    public static Message fromObject(Object msgContents) {
        Message retVal = new Message();
        if (msgContents instanceof JsonObject msgJsonObj)
        {
            if (msgJsonObj.has("header"))
            {
                retVal.header = MessageHeader.fromDict(msgJsonObj.getAsJsonObject("header"));
            }
            if (msgJsonObj.has("tags"))
            {
                retVal.tags = MessageTags.fromDict(msgJsonObj.getAsJsonObject("tags"));
            }
            if (msgJsonObj.has("body"))
            {
                retVal.body = msgJsonObj.get("body");
            }
            if (!(msgJsonObj.has("header") || msgJsonObj.has("tags") || msgJsonObj.has("body")))
            {
                retVal.raw = msgJsonObj;
            }
        }
        else
        {
            retVal.raw = msgContents;
        }
        return retVal;
    }
    

    
    public MessageBuilder withCorrelationId(String correlationId) {
        this.correlationId = correlationId;
        return this;
    }
    
    public MessageBuilder withPayload(Object payload) {
        this.payload = payload;
        return this;
    }
    
    public MessageBuilder withConfig(ConfigManager configService) {
        this.configService = configService;
        return this;
    }
    
    public Message build() {
        if (configService == null) {
            throw new IllegalStateException("Configuration service is required - call withConfig()");
        }
        
        Message message = new Message();
        MessageHeaderBuilder headerBuilder = MessageHeaderBuilder.create(name, version);
        if (correlationId != null) {
            headerBuilder.withCorrelationId(correlationId);
        }
        message.header = headerBuilder.build();
        message.tags = MessageTags.fromConfig(configService);
        
        if (payload instanceof String payloadStr) {
            try {
                Gson gson = new Gson();
                message.body = gson.fromJson(payloadStr, Object.class);
            } catch (JsonSyntaxException e) {
                message.body = payloadStr;
            }
        } else {
            message.body = payload;
        }
        
        return message;
    }
}