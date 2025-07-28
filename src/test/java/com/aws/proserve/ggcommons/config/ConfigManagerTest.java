/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.config;

import com.aws.proserve.ggcommons.ParsedCommandLine;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;
import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for ConfigManager class.
 * Tests actual configuration loading and parsing with sample config files.
 */
class ConfigManagerTest {
    
    @Test
    void testBasicConfigurationLoading() throws IOException {
        String configJson = "{" +
            "\"logging\": {\"level\": \"INFO\"}," +
            "\"metricEmission\": {\"target\": \"log\"}," +
            "\"heartbeat\": {\"intervalSecs\": 30}," +
            "\"tags\": {}," +
            "\"component\": {\"global\": {\"timeout\": 5000}}" +
        "}";
        
        runWithTempConfig(configJson, configManager -> {
            assertNotNull(configManager.getGlobalConfig());
            assertEquals(5000, configManager.getGlobalConfig().get("timeout").getAsInt());
        });
    }
    
    @Test
    void testTemplateResolution() throws IOException {
        String configJson = "{" +
            "\"logging\": {\"level\": \"INFO\"}," +
            "\"metricEmission\": {\"target\": \"log\"}," +
            "\"heartbeat\": {\"intervalSecs\": 30}," +
            "\"tags\": {\"environment\": \"production\", \"region\": \"us-west-2\"}," +
            "\"component\": {\"global\": {}}" +
        "}";
        
        runWithTempConfig(configJson, configManager -> {
            String template = "Log path: /var/log/{ComponentName}-{ThingName}-{environment}.log";
            String resolved = configManager.resolveTemplate(template);
            
            assertTrue(resolved.contains("TestComponent"));
            assertTrue(resolved.contains("test-thing"));
            assertTrue(resolved.contains("production"));
            assertFalse(resolved.contains("{ComponentName}"));
            assertFalse(resolved.contains("{ThingName}"));
            assertFalse(resolved.contains("{environment}"));
        });
    }
    
    @Test
    void testMultipleInstanceConfiguration() throws IOException {
        String configJson = "{" +
            "\"logging\": {\"level\": \"INFO\"}," +
            "\"metricEmission\": {\"target\": \"log\"}," +
            "\"heartbeat\": {\"intervalSecs\": 30}," +
            "\"tags\": {}," +
            "\"component\": {" +
                "\"global\": {\"serverUrl\": \"https://api.example.com\"}," +
                "\"instances\": [" +
                    "{\"id\": \"instance1\", \"database\": {\"host\": \"db1.local\", \"port\": 5432}}," +
                    "{\"id\": \"instance2\", \"database\": {\"host\": \"db2.local\", \"port\": 5433}}" +
                "]" +
            "}" +
        "}";
        
        runWithTempConfig(configJson, configManager -> {
            assertEquals(2, configManager.getInstanceIds().size());
            assertTrue(configManager.getInstanceIds().contains("instance1"));
            assertTrue(configManager.getInstanceIds().contains("instance2"));
            
            JsonObject instance1 = configManager.getInstanceConfig("instance1");
            assertEquals("db1.local", instance1.getAsJsonObject("database").get("host").getAsString());
            assertEquals(5432, instance1.getAsJsonObject("database").get("port").getAsInt());
        });
    }
    
    @Test
    void testConfigurationChangeListeners() throws IOException {
        String configJson = "{" +
            "\"logging\": {\"level\": \"INFO\"}," +
            "\"metricEmission\": {\"target\": \"log\"}," +
            "\"heartbeat\": {\"intervalSecs\": 30}," +
            "\"tags\": {}," +
            "\"component\": {\"global\": {\"value\": 100}}" +
        "}";
        
        runWithTempConfig(configJson, configManager -> {
            configManager.completeInitialization();
            
            TestConfigurationChangeListener listener = new TestConfigurationChangeListener();
            configManager.addConfigChangeListener(listener);
            
            configManager.notifyConfigurationChanged();
            assertTrue(listener.wasOnConfigurationChangedCalled());
            
            configManager.removeConfigChangeListener(listener);
            listener.reset();
            configManager.notifyConfigurationChanged();
            assertFalse(listener.wasOnConfigurationChangedCalled());
        });
    }
    
    @Test
    void testMetricConfiguration() throws IOException {
        String configJson = "{" +
            "\"logging\": {\"level\": \"INFO\"}," +
            "\"metricEmission\": {\"target\": \"cloudwatch\", \"namespace\": \"TestNamespace\", \"targetConfig\": {\"intervalSecs\": 30}}," +
            "\"heartbeat\": {\"intervalSecs\": 30}," +
            "\"tags\": {}," +
            "\"component\": {\"global\": {}}" +
        "}";
        
        runWithTempConfig(configJson, configManager -> {
            MetricConfiguration metricConfig = configManager.getMetricConfig();
            assertNotNull(metricConfig);
            assertEquals("cloudwatch", metricConfig.getTarget());
            assertEquals("TestNamespace", metricConfig.getNamespace());
            assertEquals(30, metricConfig.getIntervalSecs());
        });
    }
    
    @Test
    void testMetricConfigurationWithFileTarget() throws IOException {
        String configJson = "{" +
            "\"logging\": {\"level\": \"INFO\"}," +
            "\"metricEmission\": {\"target\": \"log\", \"targetConfig\": {\"logFileName\": \"/var/log/metrics.log\", \"maxFileSize\": \"100MB\"}}," +
            "\"heartbeat\": {\"intervalSecs\": 30}," +
            "\"tags\": {}," +
            "\"component\": {\"global\": {}}" +
        "}";
        
        runWithTempConfig(configJson, configManager -> {
            MetricConfiguration metricConfig = configManager.getMetricConfig();
            assertEquals("log", metricConfig.getTarget());
            assertEquals("/var/log/metrics.log", metricConfig.getLogFileNameTemplate());
            assertEquals("100MB", metricConfig.getMaxFileSize());
        });
    }
    
    @Test
    void testLoggingConfiguration() throws IOException {
        String configJson = "{" +
            "\"logging\": {\"level\": \"DEBUG\", \"format\": \"%d{yyyy-MM-dd HH:mm:ss} [%level] %logger{36} - %msg%n\", \"fileLogging\": {\"enabled\": true, \"filePath\": \"/var/log/{ComponentName}.log\"}, \"loggers\": {\"com.aws.proserve\": \"INFO\", \"org.apache.http\": \"WARN\"}, \"globalControl\": true}," +
            "\"metricEmission\": {\"target\": \"log\"}," +
            "\"heartbeat\": {\"intervalSecs\": 30}," +
            "\"tags\": {}," +
            "\"component\": {\"global\": {}}" +
        "}";
        
        runWithTempConfig(configJson, configManager -> {
            LoggingConfiguration loggingConfig = configManager.getLoggingConfig();
            assertNotNull(loggingConfig);
            assertEquals("DEBUG", loggingConfig.getLevel().toString());
            assertTrue(loggingConfig.isFileLoggingEnabled());
            assertEquals("/var/log/{ComponentName}.log", loggingConfig.getLogFilePath());
            assertEquals("%d{yyyy-MM-dd HH:mm:ss} [%level] %logger{36} - %msg%n", loggingConfig.getFormat());
            assertTrue(loggingConfig.isGlobalControlEnabled());
            assertEquals(2, loggingConfig.getLoggerLevels().size());
            assertEquals("INFO", loggingConfig.getLoggerLevels().get("com.aws.proserve").toString());
            assertEquals("WARN", loggingConfig.getLoggerLevels().get("org.apache.http").toString());
        });
    }
    
    @Test
    void testHeartbeatConfiguration() throws IOException {
        String configJson = "{" +
            "\"logging\": {\"level\": \"INFO\"}," +
            "\"metricEmission\": {\"target\": \"log\"}," +
            "\"heartbeat\": {\"intervalSecs\": 60, \"targets\": [{\"type\": \"metric\"}, {\"type\": \"messaging\", \"topic\": \"heartbeat/status\"}]}," +
            "\"tags\": {}," +
            "\"component\": {\"global\": {}}" +
        "}";
        
        runWithTempConfig(configJson, configManager -> {
            HeartbeatConfiguration heartbeatConfig = configManager.getHeartbeatConfig();
            assertNotNull(heartbeatConfig);
            assertEquals(60, heartbeatConfig.getIntervalSecs());
            assertNotNull(heartbeatConfig.getTargets());
            assertEquals(2, heartbeatConfig.getTargets().size());
        });
    }
    
    @Test
    void testTagConfiguration() throws IOException {
        String configJson = "{" +
            "\"logging\": {\"level\": \"INFO\"}," +
            "\"metricEmission\": {\"target\": \"log\"}," +
            "\"heartbeat\": {\"intervalSecs\": 30}," +
            "\"tags\": {\"environment\": \"production\", \"region\": \"us-west-2\", \"service\": \"data-processor\", \"version\": \"1.2.3\"}," +
            "\"component\": {\"global\": {}}" +
        "}";
        
        runWithTempConfig(configJson, configManager -> {
            TagConfiguration tagConfig = configManager.getTagConfig();
            assertNotNull(tagConfig);
            assertNotNull(tagConfig.getKeys());
            assertTrue(tagConfig.getKeys().contains("environment"));
            assertTrue(tagConfig.getKeys().contains("region"));
            assertEquals("production", tagConfig.getKeyValue("environment"));
            assertEquals("us-west-2", tagConfig.getKeyValue("region"));
            assertEquals("data-processor", tagConfig.getKeyValue("service"));
            assertEquals("1.2.3", tagConfig.getKeyValue("version"));
        });
    }
    
    @Test
    void testComplexTemplateResolutionWithAllVariables() throws IOException {
        String configJson = "{" +
            "\"logging\": {\"level\": \"INFO\"}," +
            "\"metricEmission\": {\"target\": \"log\"}," +
            "\"heartbeat\": {\"intervalSecs\": 30}," +
            "\"tags\": {\"environment\": \"staging\", \"datacenter\": \"dc1\"}," +
            "\"component\": {\"global\": {}}" +
        "}";
        
        runWithTempConfig(configJson, configManager -> {
            String template = "Path: /opt/{ComponentFullName}/{ComponentName}-{ThingName}-{environment}-{datacenter}/data";
            String resolved = configManager.resolveTemplate(template);
            
            assertTrue(resolved.contains("com.test.TestComponent"));
            assertTrue(resolved.contains("TestComponent"));
            assertTrue(resolved.contains("test-thing"));
            assertTrue(resolved.contains("staging"));
            assertTrue(resolved.contains("dc1"));
            assertFalse(resolved.contains("{ComponentFullName}"));
            assertFalse(resolved.contains("{ComponentName}"));
            assertFalse(resolved.contains("{ThingName}"));
            assertFalse(resolved.contains("{environment}"));
            assertFalse(resolved.contains("{datacenter}"));
        });
    }
    
    @Test
    void testComponentNameParsing() throws IOException {
        String configJson = "{" +
            "\"logging\": {\"level\": \"INFO\"}," +
            "\"metricEmission\": {\"target\": \"log\"}," +
            "\"heartbeat\": {\"intervalSecs\": 30}," +
            "\"tags\": {}," +
            "\"component\": {\"global\": {}}" +
        "}";
        
        runWithTempConfig(configJson, configManager -> {
            assertEquals("com.test.TestComponent", configManager.getComponentFullName());
            assertEquals("TestComponent", configManager.getComponentName());
            assertEquals("test-thing", configManager.getThingName());
        });
    }
    
    @Test
    void testFullConfigurationAccess() throws IOException {
        String configJson = "{" +
            "\"logging\": {\"level\": \"INFO\"}," +
            "\"metricEmission\": {\"target\": \"cloudwatch\"}," +
            "\"heartbeat\": {\"intervalSecs\": 30}," +
            "\"tags\": {\"env\": \"test\"}," +
            "\"component\": {\"global\": {\"timeout\": 5000}}" +
        "}";
        
        runWithTempConfig(configJson, configManager -> {
            JsonObject fullConfig = configManager.getFullConfig();
            assertNotNull(fullConfig);
            assertTrue(fullConfig.has("logging"));
            assertTrue(fullConfig.has("metricEmission"));
            assertTrue(fullConfig.has("heartbeat"));
            assertTrue(fullConfig.has("tags"));
            assertTrue(fullConfig.has("component"));
        });
    }
    
    private File createTempConfig(String configJson) throws IOException {
        File tempFile = File.createTempFile("test-config", ".json");
        tempFile.deleteOnExit(); // Ensure cleanup if test fails
        try (FileWriter writer = new FileWriter(tempFile)) {
            writer.write(configJson);
            writer.flush(); // Ensure data is written to disk
        }
        return tempFile;
    }
    
    private ConfigManager createConfigManager(String configPath) {
        ParsedCommandLine cmdLine = new ParsedCommandLine();
        cmdLine.configArgs = new String[]{"FILE", configPath};
        cmdLine.thingName = "test-thing";
        return new ConfigManager("com.test.TestComponent", cmdLine);
    }
    
    private void runWithTempConfig(String configJson, ConfigTest test) throws IOException {
        File tempConfigFile = createTempConfig(configJson);
        ConfigManager configManager = createConfigManager(tempConfigFile.getAbsolutePath());
        test.run(configManager);
        // Don't delete here - let deleteOnExit() handle cleanup
    }
    
    @FunctionalInterface
    private interface ConfigTest {
        void run(ConfigManager configManager) throws IOException;
    }
    
    private static class TestConfigurationChangeListener implements ConfigurationChangeListener {
        private boolean onConfigurationChangedCalled = false;
        
        @Override
        public boolean onConfigurationChanged() {
            onConfigurationChangedCalled = true;
            return true;
        }
        
        public boolean wasOnConfigurationChangedCalled() {
            return onConfigurationChangedCalled;
        }
        
        public void reset() {
            onConfigurationChangedCalled = false;
        }
    }
}