package com.mbreissi.edgecommons.messaging;

import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.messaging.proto.MessageBodyCase;
import com.mbreissi.edgecommons.messaging.proto.MessageBodySchema;
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
    private String uuid;
    private String timestamp;
    private Long timestampMs;
    private String replyTo;
    private Object payload;
    private String contentType;
    private String contentEncoding;
    private MessageBodySchema schema;
    private MessageBodyCase bodyCase;
    private MessageTags tagsOverride;
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
            if (msgJsonObj.has("content_type"))
            {
                retVal.contentType = msgJsonObj.get("content_type").getAsString();
            }
            if (msgJsonObj.has("content_encoding"))
            {
                retVal.contentEncoding = msgJsonObj.get("content_encoding").getAsString();
            }
            if (msgJsonObj.has("schema"))
            {
                retVal.schema = MessageBodySchema.fromDict(msgJsonObj.getAsJsonObject("schema"));
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

    /**
     * Pins the header {@code uuid} instead of the generated random one — deterministic envelopes
     * for tests and the cross-language {@code uns-test-vectors} golden envelopes (D-U13).
     *
     * @param uuid the header uuid to stamp verbatim
     */
    public MessageBuilder withUuid(String uuid) {
        this.uuid = uuid;
        return this;
    }

    /**
     * Pins the header {@code timestamp} instead of the generated "now" — deterministic envelopes
     * for tests and the cross-language {@code uns-test-vectors} golden envelopes (D-U13).
     *
     * @param timestamp the header timestamp to stamp verbatim (ISO-8601 instant by convention)
     */
    public MessageBuilder withTimestamp(String timestamp) {
        this.timestamp = timestamp;
        return this;
    }

    public MessageBuilder withTimestampMs(long timestampMs) {
        this.timestampMs = timestampMs;
        return this;
    }

    public MessageBuilder withReplyTo(String replyTo) {
        this.replyTo = replyTo;
        return this;
    }

    public MessageBuilder withPayload(Object payload) {
        this.payload = payload;
        if (payload instanceof byte[] && this.bodyCase == null) {
            this.bodyCase = MessageBodyCase.OPAQUE;
            if (this.contentType == null) {
                this.contentType = "application/octet-stream";
            }
        }
        return this;
    }

    public MessageBuilder withStructuredPayload(Object payload) {
        this.payload = payload;
        this.bodyCase = MessageBodyCase.STRUCTURED;
        return this;
    }

    public MessageBuilder withStructuredBody(Object body) {
        return withStructuredPayload(body);
    }

    public MessageBuilder withSouthboundSignalUpdate(JsonObject payload) {
        this.payload = payload;
        this.bodyCase = MessageBodyCase.SOUTHBOUND_SIGNAL_UPDATE;
        return this;
    }

    public MessageBuilder withStateUpdate(JsonObject payload) {
        this.payload = payload;
        this.bodyCase = MessageBodyCase.STATE_UPDATE;
        return this;
    }

    public MessageBuilder withConfigUpdate(JsonObject payload) {
        this.payload = payload;
        this.bodyCase = MessageBodyCase.CONFIG_UPDATE;
        return this;
    }

    public MessageBuilder withMetricUpdate(JsonObject payload) {
        this.payload = payload;
        this.bodyCase = MessageBodyCase.METRIC_UPDATE;
        return this;
    }

    public MessageBuilder withEvent(JsonObject payload) {
        this.payload = payload;
        this.bodyCase = MessageBodyCase.EVENT;
        return this;
    }

    public MessageBuilder withCommand(JsonObject payload) {
        this.payload = payload;
        this.bodyCase = MessageBodyCase.COMMAND;
        return this;
    }

    public MessageBuilder withOpaquePayload(byte[] payload) {
        return withOpaquePayload(payload, "application/octet-stream");
    }

    public MessageBuilder withOpaquePayload(byte[] payload, String contentType) {
        this.payload = payload == null ? null : payload.clone();
        this.bodyCase = MessageBodyCase.OPAQUE;
        this.contentType = contentType != null ? contentType : "application/octet-stream";
        return this;
    }

    public MessageBuilder withOpaqueBody(byte[] payload) {
        return withOpaquePayload(payload);
    }

    public MessageBuilder withOpaqueBody(byte[] payload, String contentType) {
        return withOpaquePayload(payload, contentType);
    }

    public MessageBuilder withContentType(String contentType) {
        this.contentType = contentType;
        return this;
    }

    public MessageBuilder withContentEncoding(String contentEncoding) {
        this.contentEncoding = contentEncoding;
        return this;
    }

    public MessageBuilder withSchema(MessageBodySchema schema) {
        this.schema = schema;
        return this;
    }

    public MessageBuilder withBodyCase(MessageBodyCase bodyCase) {
        this.bodyCase = bodyCase;
        return this;
    }

    public MessageBuilder withTags(MessageTags tags) {
        this.tagsOverride = tags;
        return this;
    }

    public MessageBuilder withConfig(ConfigManager configService) {
        this.configService = configService;
        return this;
    }

    /**
     * Sets the per-message instance token stamped into the identity element. A {@code null}/empty
     * token means component scope (D‑U28: the identity carries no instance key). Only takes effect
     * when an identity is stamped (a config service is present or an explicit identity override
     * carries it).
     *
     * @param instance the instance token, or {@code null}/empty for component/global scope
     */
    public MessageBuilder withInstance(String instance) {
        this.instance = (instance == null || instance.isEmpty()) ? null : instance;
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
        if (uuid != null) {
            headerBuilder.withUuid(uuid);
        }
        if (timestamp != null) {
            headerBuilder.withTimestamp(timestamp);
        }
        if (timestampMs != null) {
            headerBuilder.withTimestampMs(timestampMs);
        }
        if (replyTo != null) {
            headerBuilder.withReplyTo(replyTo);
        }
        message.header = headerBuilder.build();
        if (tagsOverride != null) {
            message.tags = tagsOverride;
        } else if (configService != null) {
            message.tags = MessageTags.fromConfig(configService);
        }

        // The single identity stamping site: explicit override > config-resolved component
        // identity (+ per-message instance token) > none (bootstrap/raw cases stay valid).
        if (identityOverride != null) {
            message.identity = identityOverride;
        } else if (configService != null) {
            MessageIdentity componentIdentity = configService.getComponentIdentity();
            if (componentIdentity != null) {
                message.identity = componentIdentity.withInstance(instance);   // D‑U28: null ⇒ component scope
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
        message.contentType = contentType;
        message.contentEncoding = contentEncoding;
        message.schema = schema;
        message.bodyCase = bodyCase;

        return message;
    }
}
