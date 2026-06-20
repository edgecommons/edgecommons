package com.aws.proserve.ggcommons.messaging;

import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;

class MessageHeaderBuilderTest {

    @Test
    void testBasicBuilder() {
        MessageHeader header = MessageHeaderBuilder.create("TestMessage", "1.0").build();
        
        assertNotNull(header);
        assertEquals("TestMessage", header.getName());
        assertEquals("1.0", header.getVersion());
        assertNotNull(header.getCorrelationId());
        assertNotNull(header.getTimestamp());
    }
    
    @Test
    void testBuilderWithCorrelationId() {
        String correlationId = "test-correlation-123";
        MessageHeader header = MessageHeaderBuilder.create("TestMessage", "1.0")
                .withCorrelationId(correlationId)
                .build();
        
        assertEquals(correlationId, header.getCorrelationId());
    }
    
    @Test
    void testBuilderWithReplyTo() {
        String replyTo = "test/reply/topic";
        MessageHeader header = MessageHeaderBuilder.create("TestMessage", "1.0")
                .withReplyTo(replyTo)
                .build();
        
        assertEquals(replyTo, header.getReplyTo());
    }
    
    @Test
    void testBuilderChaining() {
        MessageHeader header = MessageHeaderBuilder.create("TestMessage", "1.0")
                .withCorrelationId("test-123")
                .withReplyTo("test/reply")
                .withUuid("uuid-123")
                .build();
        
        assertNotNull(header);
        assertEquals("test-123", header.getCorrelationId());
        assertEquals("test/reply", header.getReplyTo());
    }
    
    @Test
    void testBuilderValidation() {
        assertThrows(IllegalArgumentException.class, () -> 
            MessageHeaderBuilder.create(null, "1.0"));
        assertThrows(IllegalArgumentException.class, () -> 
            MessageHeaderBuilder.create("Test", null));
    }
}