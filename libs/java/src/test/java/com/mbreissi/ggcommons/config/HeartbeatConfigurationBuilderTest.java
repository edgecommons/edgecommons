package com.mbreissi.ggcommons.config;

import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;

class HeartbeatConfigurationBuilderTest {

    @Test
    void testBasicBuilder() {
        HeartbeatConfiguration config = HeartbeatConfigurationBuilder.create()
                .withInterval(10)
                .build();
        
        assertNotNull(config);
        assertEquals(10, config.getIntervalSecs());
    }
    
    @Test
    void testMeasureConfiguration() {
        HeartbeatConfiguration config = HeartbeatConfigurationBuilder.create()
                .includeCpu(true)
                .includeMemory(false)
                .includeThreads(true)
                .includeFiles(false)
                .build();
        
        assertTrue(config.includeCpu());
        assertFalse(config.includeMemory());
        assertTrue(config.includeThreads());
        assertFalse(config.includeFiles());
    }
    
    @Test
    void testTargets() {
        HeartbeatConfiguration config = HeartbeatConfigurationBuilder.create()
                .addMetricTarget()
                .addMessagingTarget("heartbeat/topic", "ipc")
                .build();
        
        assertEquals(2, config.getTargets().size());
        assertEquals("metric", config.getTargets().get(0).getType());
        assertEquals("messaging", config.getTargets().get(1).getType());
    }
    
    @Test
    void testBuilderChaining() {
        HeartbeatConfiguration config = HeartbeatConfigurationBuilder.create()
                .withInterval(30)
                .includeCpu(true)
                .includeMemory(true)
                .includeThreads(false)
                .addMetricTarget()
                .addMessagingTarget("test/heartbeat", "iotcore")
                .build();
        
        assertNotNull(config);
        assertEquals(30, config.getIntervalSecs());
        assertTrue(config.includeCpu());
        assertTrue(config.includeMemory());
        assertFalse(config.includeThreads());
        assertEquals(2, config.getTargets().size());
    }
    
    @Test
    void testIntervalValidation() {
        assertThrows(IllegalArgumentException.class, () -> 
            HeartbeatConfigurationBuilder.create().withInterval(0));
        assertThrows(IllegalArgumentException.class, () -> 
            HeartbeatConfigurationBuilder.create().withInterval(-1));
    }
}