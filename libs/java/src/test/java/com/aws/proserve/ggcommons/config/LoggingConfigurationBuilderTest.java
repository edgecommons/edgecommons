package com.aws.proserve.ggcommons.config;

import org.apache.logging.log4j.Level;
import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;

class LoggingConfigurationBuilderTest {

    @Test
    void testBasicBuilder() {
        LoggingConfiguration config = LoggingConfigurationBuilder.create()
                .withLevel(Level.DEBUG)
                .build();
        
        assertNotNull(config);
        assertEquals(Level.DEBUG, config.getLevel());
    }
    
    @Test
    void testFileLogging() {
        LoggingConfiguration config = LoggingConfigurationBuilder.create()
                .enableFileLogging("/tmp/test.log")
                .build();
        
        assertTrue(config.isFileLoggingEnabled());
        assertEquals("/tmp/test.log", config.getLogFilePath());
    }
    
    @Test
    void testLoggerLevels() {
        LoggingConfiguration config = LoggingConfigurationBuilder.create()
                .addLogger("com.example", Level.WARN)
                .addLogger("com.test", "ERROR")
                .build();
        
        assertEquals(2, config.getLoggerLevels().size());
        assertEquals(Level.WARN, config.getLoggerLevels().get("com.example"));
        assertEquals(Level.ERROR, config.getLoggerLevels().get("com.test"));
    }
    
    @Test
    void testBuilderChaining() {
        LoggingConfiguration config = LoggingConfigurationBuilder.create()
                .withLevel(Level.INFO)
                .withFormat("%d [%p] %c: %m%n")
                .enableFileLogging("/var/log/app.log")
                .addLogger("com.example", Level.DEBUG)
                .enableGlobalControl()
                .build();
        
        assertNotNull(config);
        assertEquals(Level.INFO, config.getLevel());
        assertTrue(config.isFileLoggingEnabled());
        assertTrue(config.isGlobalControlEnabled());
        assertEquals(1, config.getLoggerLevels().size());
    }
    
    @Test
    void testDisableFileLogging() {
        LoggingConfiguration config = LoggingConfigurationBuilder.create()
                .enableFileLogging("/tmp/test.log")
                .disableFileLogging()
                .build();
        
        assertFalse(config.isFileLoggingEnabled());
        assertNull(config.getLogFilePath());
    }
}