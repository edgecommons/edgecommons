package com.mbreissi.ggcommons.messaging;

import com.mbreissi.ggcommons.config.ConfigManager;
import com.google.gson.Gson;
import com.google.gson.JsonObject;
import com.google.gson.JsonSyntaxException;

/**
 * Builder for creating Message instances with fluent API.
 *
 * <p>{@link #build()} is the single UNS identity stamping site: an explicit
 * {@link #withIdentity(MessageIdentity)} override wins; otherwise, when a config service is
 * present, the component's resolved identity ({@code ConfigManager.getComponentIdentity()}) is
 * stamped with the per-message instance token ({@link #withInstance(String)}, default
 * {@value MessageIdentity#DEFAULT_INSTANCE}); with neither, {@code identity} stays {@code null}
 * (bootstrap/raw messages legally omit it).
 */
public class MessageBuilder {
    private String name;
    private String version;
    private String correlationId;
    private Object payload;
    private ConfigManager configService;
    private String instance;
    private MessageIdentity identityOverride;

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
            if (msgJsonObj.has("identity"))
            {
                retVal.identity = Message.parseIdentity(msgJsonObj.get("identity"));
            }
            if (msgJsonObj.has("tags"))
            {
                retVal.tags = MessageTags.fromDict(msgJsonObj.getAsJsonObject("tags"));
            }
            if (msgJsonObj.has("body"))
            {
                retVal.body = msgJsonObj.get("body");
            }
            if (!(msgJsonObj.has("header") || msgJsonObj.has("identity")
                    || msgJsonObj.has("tags") || msgJsonObj.has("body")))
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

    /**
     * Sets the per-message instance token stamped into the identity element (default
     * {@value MessageIdentity#DEFAULT_INSTANCE}). Only takes effect when an identity is stamped
     * (a config service is present or an explicit identity override carries it).
     *
     * @param instance the instance token (non-null, non-empty)
     * @throws IllegalArgumentException if {@code instance} is null or empty
     */
    public MessageBuilder withInstance(String instance) {
        if (instance == null || instance.isEmpty()) {
            throw new IllegalArgumentException("instance must be non-empty");
        }
        this.instance = instance;
        return this;
    }

    /**
     * Sets an explicit identity override (tests, conformance vectors, relays). Wins over the
     * config-resolved identity and is stamped verbatim (the {@link #withInstance(String)} token
     * is not applied to an override).
     *
     * @param identity the identity to stamp
     */
    public MessageBuilder withIdentity(MessageIdentity identity) {
        this.identityOverride = identity;
        return this;
    }

    public Message build() {
        Message message = new Message();
        MessageHeaderBuilder headerBuilder = MessageHeaderBuilder.create(name, version);
        if (correlationId != null) {
            headerBuilder.withCorrelationId(correlationId);
        }
        message.header = headerBuilder.build();
        if (configService != null) {
            message.tags = MessageTags.fromConfig(configService);
        }

        // The single identity stamping site: explicit override > config-resolved component
        // identity (+ per-message instance token) > none (bootstrap/raw cases stay valid).
        if (identityOverride != null) {
            message.identity = identityOverride;
        } else if (configService != null) {
            MessageIdentity componentIdentity = configService.getComponentIdentity();
            if (componentIdentity != null) {
                message.identity = componentIdentity.withInstance(
                        instance != null ? instance : MessageIdentity.DEFAULT_INSTANCE);
            }
        }

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
