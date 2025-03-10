/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging;

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

    public MessageHeader(String name, String version)
    {
        this(name, version, null, null, null, null);
    }

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

    public static MessageHeader fromDict(JsonObject src)
    {
        String name = src.has("name") ? src.get("name").getAsString() : null;
        String version = src.has("version") ? src.get("version").getAsString() : null;
        String timestamp = src.has("timestamp") ? src.get("timestamp").getAsString() : null;
        String uuid = src.has("uuid") ? src.get("uuid").getAsString() : null;
        String correlationId = src.has("correlation_id") ? src.get("correlation_id").getAsString() : null;
        String replyTo = src.has("reply_to") ? src.get("reply_to").getAsString() : null;
        return new MessageHeader(name, version, correlationId, timestamp, uuid, replyTo);
    }

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

    public String makeRequest(String replyTo)
    {
        this.replyTo = Objects.requireNonNullElseGet(replyTo, () -> REPLY_MESSAGE_TOPIC_PREFIX + UUID.randomUUID());
        LOGGER.debug("Setting replyTo field as {}", this.replyTo );
        return this.replyTo;
    }

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

    public void setCorrelationId(String correlationId)
    {
        this.correlationId = correlationId;
    }
}
