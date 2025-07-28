package com.aws.proserve.ggcommons.messaging;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.google.gson.Gson;
import com.google.gson.JsonSyntaxException;

/**
 * Builder for creating Message instances with fluent API.
 */
public class MessageBuilder {
    private String name;
    private String version;
    private String correlationId;
    private Object payload;
    private ConfigManager configManager;
    
    private MessageBuilder(String name, String version) {
        this.name = name;
        this.version = version;
    }
    
    public static MessageBuilder create(String name, String version) {
        return new MessageBuilder(name, version);
    }
    

    
    public MessageBuilder withCorrelationId(String correlationId) {
        this.correlationId = correlationId;
        return this;
    }
    
    public MessageBuilder withPayload(Object payload) {
        this.payload = payload;
        return this;
    }
    
    public MessageBuilder withConfig(ConfigManager configManager) {
        this.configManager = configManager;
        return this;
    }
    
    public Message build() {
        if (configManager == null) {
            throw new IllegalStateException("ConfigManager is required - call withConfig()");
        }
        
        Message message = new Message();
        message.header = new MessageHeader(name, version, correlationId);
        message.tags = MessageTags.fromConfig(configManager);
        
        if (payload instanceof String) {
            String payloadStr = (String) payload;
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