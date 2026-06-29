/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.messaging;

import com.google.gson.Gson;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.time.ZoneOffset;
import java.time.ZonedDateTime;
import java.time.format.DateTimeFormatter;
import java.util.Map;
import java.util.Objects;
import java.util.UUID;

/**
 * Represents the header portion of a message in the Greengrass messaging system.
 * Contains metadata about the message such as timestamp, correlation ID, and tags.
 */
public class MessageHeader
{
    protected static final Logger LOGGER = LogManager.getLogger(MessageHeader.class);

    static final String REPLY_MESSAGE_TOPIC_PREFIX = "ggcommons/reply-";

    String name;
    String version;
    String timestamp;
    String correlationId;
    String uuid;
    String replyTo;

    /**
     * Creates a message header with name and version.
     *
     * @param name The name of the message type
     * @param version The version of the message format
     * @deprecated Use {@link MessageHeaderBuilder#create(String, String)} instead
     */
    @Deprecated
    public MessageHeader(String name, String version)
    {
        this(name, version, null, null, null, null);
    }

    /**
     * Creates a message header with name, version, and correlation ID.
     *
     * @param name The name of the message type
     * @param version The version of the message format
     * @param correlationId The correlation ID for message tracking
     * @deprecated Use {@link MessageHeaderBuilder#create(String, String)} instead
     */
    @Deprecated
    public MessageHeader(String name, String version, String correlationId)
    {
        this(name, version, correlationId, null, null, null);
    }

    public MessageHeader(String name, String version, String correlationId, String timestamp,
                         String uuid, String replyTo)
    {
        this.name = name;
        this.version = version;
        if (timestamp == null)
            timestamp = ZonedDateTime.now(ZoneOffset.UTC).format(DateTimeFormatter.ISO_INSTANT);
        this.timestamp = timestamp;
        if (correlationId == null)
            correlationId = UUID.randomUUID().toString();
        this.correlationId = correlationId;
        if (uuid == null)
            uuid = UUID.randomUUID().toString();
        this.uuid = uuid;
        this.replyTo = replyTo;
    }

    /**
     * Creates a MessageHeader from a JSON object representation.
     *
     * @param src The source JSON object containing header data
     * @return A new MessageHeader instance
     */
    public static MessageHeader fromDict(JsonObject src)
    {
        String name = src.has("name") ? src.get("name").getAsString() : null;
        String version = src.has("version") ? src.get("version").getAsString() : null;
        String timestamp = src.has("timestamp") ? src.get("timestamp").getAsString() : null;
        String uuid = src.has("uuid") ? src.get("uuid").getAsString() : null;
        String correlationId = src.has("correlation_id") ? src.get("correlation_id").getAsString() : null;
        String replyTo = src.has("reply_to") ? src.get("reply_to").getAsString() : null;
        if (name == null || version == null) {
            return new MessageHeader(name, version, correlationId, timestamp, uuid, replyTo);
        }
        
        MessageHeaderBuilder builder = MessageHeaderBuilder.create(name, version);
        if (correlationId != null) builder.withCorrelationId(correlationId);
        if (timestamp != null) builder.withTimestamp(timestamp);
        if (uuid != null) builder.withUuid(uuid);
        if (replyTo != null) builder.withReplyTo(replyTo);
        return builder.build();
    }

    /**
     * Converts the header to its JSON object representation.
     *
     * @return JsonObject containing the header data
     */
    public JsonObject toDict()
    {
        JsonObject retVal = new JsonObject();

        retVal.addProperty("name", name);
        retVal.addProperty("version", version);
        retVal.addProperty("timestamp", timestamp);
        retVal.addProperty("uuid", uuid);
        retVal.addProperty("correlation_id", correlationId);
        if (replyTo != null)
            retVal.addProperty("reply_to", replyTo);

        return retVal;
    }

    @Override
    public String toString()
    {
        return new Gson().toJson(toDict());
    }

    /**
     * Prepares the header for a request message.
     *
     * @param replyTo The topic to send replies to
     * @return The correlation ID for the request
     */
    public String makeRequest(String replyTo)
    {
        this.replyTo = Objects.requireNonNullElseGet(replyTo, () -> REPLY_MESSAGE_TOPIC_PREFIX + UUID.randomUUID());
        LOGGER.debug("Setting replyTo field as {}", this.replyTo );
        return this.replyTo;
    }

    /**
     * Gets the reply-to topic for this message.
     *
     * @return The reply-to topic or null if not set
     */
    public String getReplyTo() {
        return replyTo;
    }

    public String getName() { return name; }

    public String getVersion() { return version; }

    public String getTimestamp() { return timestamp; }

    public String getCorrelationId() {
        if (correlationId == null)
            correlationId = UUID.randomUUID().toString();
        return correlationId;
    }

    /**
     * Sets the correlation ID for this message header.
     *
     * @param correlationId The correlation ID to set
     */
    public void setCorrelationId(String correlationId)
    {
        this.correlationId = correlationId;
    }
}
