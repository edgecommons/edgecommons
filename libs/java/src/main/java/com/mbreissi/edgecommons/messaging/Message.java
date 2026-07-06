/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import com.mbreissi.edgecommons.config.ConfigManager;
import com.google.gson.Gson;
import com.google.gson.GsonBuilder;
import com.google.gson.JsonElement;
import com.google.gson.JsonNull;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.google.gson.JsonSyntaxException;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Base64;
import java.util.Map;


/**
 * Represents a message in the Greengrass messaging system.
 * Contains message headers and payload data for communication between components
 * and with AWS IoT Core.
 */
public class Message
{
    protected static final Logger LOGGER = LogManager.getLogger(Message.class);
    public static final int MAX_BINARY_BODY_BYTES = 64 * 1024;
    private static final String BINARY_BODY_KEY = "_edgecommonsBinary";
    private static final String BINARY_ENCODING = "base64";

    /** Default serializer: omits null members (Gson default), used for POJO/record/List payloads. */
    private static final Gson DEFAULT_GSON = new Gson();
    /**
     * Null-serializing serializer used only for {@link Map}-shaped payloads, where a present key with
     * a null value is unambiguous intent ({@code map.put("k", null)}) and must serialize as JSON
     * {@code null} — at parity with a Python {@code dict} {@code None}, a TS object {@code null}, and
     * serde. POJO/record payloads deliberately keep the default omit-null behavior (an unset field is
     * ambiguous), so this is not enabled globally (#15).
     */
    private static final Gson NULL_SERIALIZING_GSON = new GsonBuilder().serializeNulls().create();

    MessageHeader header;
    MessageIdentity identity;
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
        identity = null;
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
            // Canonical envelope member order: header, identity, tags, body.
            if (identity != null)
                retVal.add("identity", identity.toDict());
            if (tags != null)
                retVal.add("tags", new Gson().toJsonTree(tags.toDict()).getAsJsonObject());
            retVal.add("body", toJsonElement(body));
        }
        else
        {
            retVal.add("raw", toJsonElement(raw));
        }
        return retVal;
    }

    /**
     * Coerces a message body/raw value to a Gson {@link JsonElement} for serialization. A value that
     * is already a {@link JsonElement} is returned as-is; {@code null} becomes {@link JsonNull}; a
     * {@code byte[]} is encoded as the first-class bounded binary body marker; a {@link Map} is converted with
     * null-valued entries preserved as JSON {@code null} (#15); any other object (POJO, {@code List},
     * primitive wrapper, etc.) is converted via Gson's default reflective adapter (which omits null
     * members). This lets callers pass a plain {@code Map}/POJO to
     * {@link MessageBuilder#withPayload(Object)} and have it serialize correctly, instead of failing
     * with a {@code ClassCastException} at publish time — at parity with the Rust/Python/TypeScript
     * libraries, which accept native maps/objects as payloads.
     *
     * @param value the body or raw value (any type, may be {@code null})
     * @return the value as a {@link JsonElement}
     */
    private static JsonElement toJsonElement(Object value)
    {
        if (value == null)
            return JsonNull.INSTANCE;
        if (value instanceof JsonElement element)
            return element;
        if (value instanceof byte[] bytes)
            return binaryBodyJson(bytes);
        if (value instanceof Map<?, ?>)
            // #15: a Map carries key presence, so a null value is an explicit JSON null (not an
            // ambiguous unset POJO field) — serialize it as such, matching Python/TS/serde. We go via
            // a JSON string (not toJsonTree) because Gson's JsonTreeWriter drops null values in NESTED
            // maps; the string path with serializeNulls preserves them at every nesting level.
            return JsonParser.parseString(NULL_SERIALIZING_GSON.toJson(value));
        return DEFAULT_GSON.toJsonTree(value);
    }

    private static JsonObject binaryBodyJson(byte[] bytes)
    {
        if (bytes.length > MAX_BINARY_BODY_BYTES)
            throw new IllegalArgumentException(
                "Binary message body exceeds " + MAX_BINARY_BODY_BYTES + " bytes");
        JsonObject descriptor = new JsonObject();
        descriptor.addProperty("encoding", BINARY_ENCODING);
        descriptor.addProperty("length", bytes.length);
        descriptor.addProperty("data", Base64.getEncoder().encodeToString(bytes));
        JsonObject marker = new JsonObject();
        marker.add(BINARY_BODY_KEY, descriptor);
        return marker;
    }

    private static JsonObject binaryDescriptor(Object value)
    {
        if (value instanceof JsonObject obj && obj.has(BINARY_BODY_KEY))
        {
            if (!obj.get(BINARY_BODY_KEY).isJsonObject())
                throw new IllegalArgumentException("Binary message body marker must be an object");
            return obj.getAsJsonObject(BINARY_BODY_KEY);
        }
        return null;
    }

    private static byte[] decodeBinaryDescriptor(JsonObject descriptor)
    {
        if (!descriptor.has("encoding") || !BINARY_ENCODING.equals(descriptor.get("encoding").getAsString()))
            throw new IllegalArgumentException("Binary message body encoding must be base64");
        if (!descriptor.has("length") || !descriptor.get("length").isJsonPrimitive())
            throw new IllegalArgumentException("Binary message body length is required");
        int declaredLength = descriptor.get("length").getAsInt();
        if (declaredLength < 0 || declaredLength > MAX_BINARY_BODY_BYTES)
            throw new IllegalArgumentException(
                "Binary message body exceeds " + MAX_BINARY_BODY_BYTES + " bytes");
        if (!descriptor.has("data") || !descriptor.get("data").isJsonPrimitive())
            throw new IllegalArgumentException("Binary message body data is required");
        byte[] decoded = Base64.getDecoder().decode(descriptor.get("data").getAsString());
        if (decoded.length != declaredLength)
            throw new IllegalArgumentException("Binary message body length does not match decoded data");
        return decoded;
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
     * Gets the UNS identity element of this message ({@code hier}/{@code path}/{@code component}/
     * {@code instance}), or {@code null} when the message carries none (raw messages, messages
     * built without a config-bound builder, or a malformed inbound identity).
     *
     * @return The MessageIdentity object, or {@code null}
     */
    public MessageIdentity getIdentity()
    {
        return identity;
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
            tags = new MessageTags();
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
     * Returns true when the payload is a first-class binary body.
     *
     * @return true for an outbound byte[] payload or an inbound binary marker object
     */
    public boolean isBinaryBody()
    {
        return body instanceof byte[] || (body instanceof JsonObject obj && obj.has(BINARY_BODY_KEY));
    }

    /**
     * Decodes the first-class binary message body.
     *
     * @return the decoded bytes, or null when the body is not binary
     * @throws IllegalArgumentException when the inbound binary marker is malformed or too large
     */
    public byte[] getBinaryBody()
    {
        if (body instanceof byte[] bytes)
        {
            if (bytes.length > MAX_BINARY_BODY_BYTES)
                throw new IllegalArgumentException(
                    "Binary message body exceeds " + MAX_BINARY_BODY_BYTES + " bytes");
            return bytes.clone();
        }
        JsonObject descriptor = binaryDescriptor(body);
        return descriptor == null ? null : decodeBinaryDescriptor(descriptor);
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
            header = new MessageHeader("None", "None", null, null, null, null);
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
            header = new MessageHeader("None", "None", correlationId, null, null, null);
        else
            header.setCorrelationId(correlationId);
    }

    /**
     * @deprecated Use {@link MessageBuilder#create(String, String)} instead
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
     * @deprecated Use {@link MessageBuilder#create(String, String)} instead
     */
    @Deprecated
    public static Message buildFromConfig(String name, String version, Object payload,
                                          ConfigManager configManager, String correlationId)
    {
        Message retVal = new Message();
        MessageHeaderBuilder headerBuilder = MessageHeaderBuilder.create(name, version);
        if (correlationId != null) {
            headerBuilder.withCorrelationId(correlationId);
        }
        retVal.header = headerBuilder.build();
        retVal.tags = MessageTags.fromConfig(configManager);
        if (payload instanceof String payloadStr)
        {
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
        if (msgContents instanceof JsonObject msgJsonObj)
        {
            LOGGER.trace("Message contents: {}", msgJsonObj);
            if (msgJsonObj.has("header"))
            {
                LOGGER.trace("processing header");
                retVal.header = MessageHeader.fromDict(msgJsonObj.getAsJsonObject("header"));
                LOGGER.trace("header deserialized");
            }
            if (msgJsonObj.has("identity"))
            {
                LOGGER.trace("processing identity");
                retVal.identity = parseIdentity(msgJsonObj.get("identity"));
                LOGGER.trace("identity deserialized");
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
                retVal.body = msgJsonObj.get("body");
                LOGGER.trace("body desiralized");
            }
            if (!(msgJsonObj.has("header") || msgJsonObj.has("identity")
                    || msgJsonObj.has("tags") || msgJsonObj.has("body")))
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

    /**
     * Leniently parses an envelope {@code identity} member: a non-object element or a malformed
     * identity yields {@code null} plus a WARN (the message still delivers). Shared by the
     * deprecated {@link #build(Object)} and {@link MessageBuilder#fromObject(Object)}.
     *
     * @param identityElement the raw {@code identity} JSON element
     * @return the parsed identity, or {@code null} when malformed
     */
    static MessageIdentity parseIdentity(JsonElement identityElement)
    {
        if (identityElement == null || !identityElement.isJsonObject())
        {
            LOGGER.warn("Malformed message identity: 'identity' is not an object; dropping identity");
            return null;
        }
        return MessageIdentity.fromDict(identityElement.getAsJsonObject());
    }

}
