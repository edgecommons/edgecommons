package com.mbreissi.ggcommons.messaging;

import java.time.ZoneOffset;
import java.time.ZonedDateTime;
import java.time.format.DateTimeFormatter;
import java.util.UUID;

/**
 * Builder for creating MessageHeader instances with fluent API.
 */
public class MessageHeaderBuilder {
    private String name;
    private String version;
    private String correlationId;
    private String timestamp;
    private String uuid;
    private String replyTo;
    
    private MessageHeaderBuilder(String name, String version) {
        this.name = name;
        this.version = version;
    }
    
    public static MessageHeaderBuilder create(String name, String version) {
        if (name == null || version == null) {
            throw new IllegalArgumentException("Name and version are required");
        }
        return new MessageHeaderBuilder(name, version);
    }
    
    public MessageHeaderBuilder withCorrelationId(String correlationId) {
        this.correlationId = correlationId;
        return this;
    }
    
    public MessageHeaderBuilder withTimestamp(String timestamp) {
        this.timestamp = timestamp;
        return this;
    }
    
    public MessageHeaderBuilder withUuid(String uuid) {
        this.uuid = uuid;
        return this;
    }
    
    public MessageHeaderBuilder withReplyTo(String replyTo) {
        this.replyTo = replyTo;
        return this;
    }
    
    public MessageHeader build() {
        // Apply defaults if not set
        if (timestamp == null) {
            timestamp = ZonedDateTime.now(ZoneOffset.UTC).format(DateTimeFormatter.ISO_INSTANT);
        }
        if (correlationId == null) {
            correlationId = UUID.randomUUID().toString();
        }
        if (uuid == null) {
            uuid = UUID.randomUUID().toString();
        }
        
        return new MessageHeader(name, version, correlationId, timestamp, uuid, replyTo);
    }
}