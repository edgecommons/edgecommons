package com.aws.proserve.ggcommons.messaging;

import com.github.cliftonlabs.json_simple.JsonObject;
import com.github.cliftonlabs.json_simple.Jsoner;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.time.ZoneOffset;
import java.time.ZonedDateTime;
import java.time.format.DateTimeFormatter;
import java.util.Map;
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

    public static MessageHeader fromDict(Map<String, Object> src)
    {
        String name = (String) src.get("name");
        String version = (String) src.get("version");
        String timestamp = (String) src.get("timestamp");
        String uuid = (String) src.get("uuid");
        String correlationId = (String) src.get("correlation_id");
        String replyTo = null;
        if (src.containsKey("reply_to"))
            replyTo = (String) src.get("reply_to");
        return new MessageHeader(name, version, correlationId, timestamp, uuid, replyTo);
    }

    public JsonObject toDict()
    {
        JsonObject retVal = new JsonObject();

        retVal.put("name", name);
        retVal.put("version", version);
        retVal.put("timestamp", timestamp);
        retVal.put("uuid", uuid);
        retVal.put("correlation_id", correlationId);
        if (replyTo != null)
            retVal.put("reply_to", replyTo);

        return retVal;
    }

    @Override
    public String toString()
    {
        return Jsoner.serialize(toDict());
    }

    public String makeRequest(String replyTo)
    {
        if (replyTo == null)
            this.replyTo = REPLY_MESSAGE_TOPIC_PREFIX + UUID.randomUUID();
        else
            this.replyTo = replyTo;
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
