package com.mbreissi.edgecommons.config;

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
    void testEnabledAndDestination() {
        // The §4.3 heartbeat shape: enabled/intervalSecs/measures/destination — targets[] is gone.
        HeartbeatConfiguration config = HeartbeatConfigurationBuilder.create()
                .withEnabled(false)
                .withDestination("northbound")
                .build();

        assertFalse(config.isEnabled());
        assertEquals("northbound", config.getDestination());
    }

    @Test
    void testDefaultsAreOnFiveSecondsLocal() {
        // D-U14/M11: the heartbeat defaults are on / 5 s / local.
        HeartbeatConfiguration config = HeartbeatConfigurationBuilder.create().build();
        assertTrue(config.isEnabled());
        assertEquals(5, config.getIntervalSecs());
        assertEquals("local", config.getDestination());
    }

    @Test
    void testBuilderChaining() {
        HeartbeatConfiguration config = HeartbeatConfigurationBuilder.create()
                .withInterval(30)
                .includeCpu(true)
                .includeMemory(true)
                .includeThreads(false)
                .withEnabled(true)
                .withDestination("local")
                .build();

        assertNotNull(config);
        assertEquals(30, config.getIntervalSecs());
        assertTrue(config.includeCpu());
        assertTrue(config.includeMemory());
        assertFalse(config.includeThreads());
        assertTrue(config.isEnabled());
        assertEquals("local", config.getDestination());
    }

    @Test
    void testIntervalValidation() {
        assertThrows(IllegalArgumentException.class, () ->
            HeartbeatConfigurationBuilder.create().withInterval(0));
        assertThrows(IllegalArgumentException.class, () ->
            HeartbeatConfigurationBuilder.create().withInterval(-1));
    }

    @Test
    void testDestinationValidation() {
        assertThrows(IllegalArgumentException.class, () ->
            HeartbeatConfigurationBuilder.create().withDestination("iot_core"));
        assertThrows(IllegalArgumentException.class, () ->
            HeartbeatConfigurationBuilder.create().withDestination("iotcore"));
        assertThrows(IllegalArgumentException.class, () ->
            HeartbeatConfigurationBuilder.create().withDestination("bogus"));
    }
}
