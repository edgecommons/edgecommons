/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons;

import com.aws.proserve.ggcommons.config.ConfigurationChangeListener;
import com.aws.proserve.ggcommons.interfaces.IConfigurationService;
import com.aws.proserve.ggcommons.interfaces.IMessagingService;
import com.aws.proserve.ggcommons.interfaces.IMetricService;
import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.metrics.Measure;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.aws.proserve.ggcommons.test.MockConfigurationService;
import com.aws.proserve.ggcommons.test.MockMessagingService;
import com.aws.proserve.ggcommons.test.MockMetricService;
import com.aws.proserve.ggcommons.test.TestableGGCommons;

import com.google.gson.JsonObject;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.time.Instant;
import java.time.format.DateTimeParseException;
import java.util.Collection;
import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for GGCommons library using mock services.
 * These tests do not require external dependencies like MQTT brokers.
 */
class GGCommonsUnitTest {
    
    private static GGCommons ggCommons;
    private static MockConfigurationService mockConfigService;
    private static MockMessagingService mockMessagingService;
    private static MockMetricService mockMetricService;
    
    @BeforeAll
    static void setUpClass() {
        // Create mocks first
        mockConfigService = new MockConfigurationService();
        mockMessagingService = new MockMessagingService();
        mockMetricService = new MockMetricService();
        
        // Use TestableGGCommons that injects mocks BEFORE initialization
        String[] args = {
            "-t", "test-thing",
            "-c", "FILE", "./config_2.json"
        };
        
        ggCommons = new TestableGGCommons("com.aws.proserve.test.UnitTest", args);
        
        // Override with our specific mock instances
        ggCommons.registerService(IConfigurationService.class, mockConfigService);
        ggCommons.registerService(IMessagingService.class, mockMessagingService);
        ggCommons.registerService(IMetricService.class, mockMetricService);
    }
    
    @AfterAll
    static void tearDownClass() {
        // Cleanup if needed in future
    }
    
    @BeforeEach
    void setUp() {
        // Reset mock state for each test
        if (mockMessagingService != null) mockMessagingService.reset();
        if (mockMetricService != null) mockMetricService.reset();
//        if (mockConfigService != null) mockConfigService.reset();
    }
    
    @Test
    void testServiceRegistryInitialization() {
        // Test that service registry is properly initialized
        assertNotNull(ggCommons.getServiceRegistry());
        
        // Test that services can be retrieved
        IConfigurationService configService = ggCommons.getService(IConfigurationService.class);
        IMessagingService messagingService = ggCommons.getService(IMessagingService.class);
        IMetricService metricService = ggCommons.getService(IMetricService.class);
        
        assertNotNull(configService);
        assertNotNull(messagingService);
        assertNotNull(metricService);
        
        // Test that mock services are returned
        assertSame(mockConfigService, configService);
        assertSame(mockMessagingService, messagingService);
        assertSame(mockMetricService, metricService);
    }
    
    @Test
    void testConfigurationService() {
        // Setup test configuration
        JsonObject testConfig = new JsonObject();
        JsonObject component = new JsonObject();
        JsonObject global = new JsonObject();
        global.addProperty("testProperty", "testValue");
        component.add("global", global);
        testConfig.add("component", component);
        
        mockConfigService.setFullConfig(testConfig);
        
        // Test configuration access
        JsonObject globalConfig = mockConfigService.getGlobalConfig();
        assertNotNull(globalConfig);
        assertEquals("testValue", globalConfig.get("testProperty").getAsString());
        
        // Test thing name
        mockConfigService.setThingName("unit-test-thing");
        assertEquals("unit-test-thing", mockConfigService.getThingName());
        
        // Test template resolution
        String resolved = mockConfigService.resolveTemplate("Hello {ThingName}!");
        assertEquals("Hello unit-test-thing!", resolved);
    }
    
    @Test
    void testConfigurationChangeListener() {
        TestConfigurationChangeListener listener = new TestConfigurationChangeListener();
        
        // Add listener
        mockConfigService.addConfigChangeListener(listener);
        assertFalse(listener.wasOnConfigurationChangedCalled());
        
        // Trigger configuration change
        mockConfigService.simulateConfigurationChange();
        assertTrue(listener.wasOnConfigurationChangedCalled());
        
        // Remove listener
        mockConfigService.removeConfigChangeListener(listener);
        listener.reset();
        mockConfigService.simulateConfigurationChange();
        assertFalse(listener.wasOnConfigurationChangedCalled());
    }
    
    @Test
    void testMessagingService() {
        String testTopic = "test/topic";
        JsonObject testPayload = new JsonObject();
        testPayload.addProperty("message", "test message");
        
        // Test message creation
        Message testMessage = Message.buildFromConfig("TestMessage", "1.0", testPayload, ggCommons.getConfigManager());
        assertNotNull(testMessage);
        assertEquals("TestMessage", testMessage.getHeader().getName());
        assertEquals("1.0", testMessage.getHeader().getVersion());
        
        // Test publish
        mockMessagingService.publish(testTopic, testMessage);
        assertEquals(1, mockMessagingService.getPublishedMessages().size());
        
        MockMessagingService.PublishedMessage published = mockMessagingService.getPublishedMessages().get(0);
        assertEquals(testTopic, published.topic);
        assertEquals(testMessage, published.message);
        
        // Test raw publish
        mockMessagingService.publishRaw(testTopic, testPayload);
        assertEquals(2, mockMessagingService.getPublishedMessages().size());
        
        MockMessagingService.PublishedMessage rawPublished = mockMessagingService.getPublishedMessages().get(1);
        assertEquals(testTopic, rawPublished.topic);
        assertEquals(testPayload, rawPublished.rawPayload);
    }
    
    @Test
    void testMessagingSubscription() {
        String testTopic = "test/subscription";
        TestMessageHandler handler = new TestMessageHandler();
        
        // Test subscription
        mockMessagingService.subscribe(testTopic, handler::handle, 1);
        
        // Simulate message
        JsonObject payload = new JsonObject();
        payload.addProperty("test", "data");
        Message testMessage = Message.buildFromConfig("SubTest", "1.0", payload, ggCommons.getConfigManager());
        
        mockMessagingService.simulateMessage(testTopic, testMessage);
        
        // Verify handler was called
        assertTrue(handler.wasHandlerCalled());
        assertEquals(testTopic, handler.getReceivedTopic());
        assertEquals(testMessage, handler.getReceivedMessage());
    }
    
    @Test
    void testMessagingRequestResponse() throws ExecutionException, InterruptedException, TimeoutException {
        String requestTopic = "test/request";
        JsonObject requestPayload = new JsonObject();
        requestPayload.addProperty("request", "data");
        
        Message requestMessage = Message.buildFromConfig("RequestTest", "1.0", requestPayload, ggCommons.getConfigManager());
        
        // Test request (mock returns the same message as response)
        CompletableFuture<Message> responseFuture = mockMessagingService.request(requestTopic, requestMessage);
        Message response = responseFuture.get(1000, TimeUnit.MILLISECONDS);
        
        assertNotNull(response);
        assertEquals(requestMessage.getCorrelationId(), response.getCorrelationId());
        
        // Verify request was published
        assertEquals(1, mockMessagingService.getPublishedMessages().size());
        MockMessagingService.PublishedMessage published = mockMessagingService.getPublishedMessages().get(0);
        assertEquals(requestTopic, published.topic);
        assertEquals(requestMessage, published.message);
    }
    
    @Test
    void testMetricService() {
        // Test metric definition
        Metric testMetric = new Metric("test_metric");
        testMetric.addMeasure(new Measure("count", "Count", 1));
        testMetric.addMeasure(new Measure("latency", "Milliseconds", 1));
        
        mockMetricService.defineMetric(testMetric);
        
        assertTrue(mockMetricService.isMetricDefined("test_metric"));
        assertEquals(1, mockMetricService.getDefinedMetrics().size());
        assertEquals(testMetric, mockMetricService.getDefinedMetrics().get("test_metric"));
        
        // Test metric emission
        Map<String, Float> values = Map.of(
            "count", 5.0f,
            "latency", 100.0f
        );
        
        mockMetricService.emitMetric("test_metric", values);
        assertEquals(1, mockMetricService.getEmittedMetrics().size());
        
        MockMetricService.EmittedMetric emitted = mockMetricService.getEmittedMetrics().get(0);
        assertEquals("test_metric", emitted.name);
        assertEquals(values, emitted.measureValues);
        assertFalse(emitted.immediate);
        
        // Test immediate emission
        mockMetricService.emitMetricNow("test_metric", values);
        assertEquals(2, mockMetricService.getEmittedMetrics().size());
        
        MockMetricService.EmittedMetric immediateEmitted = mockMetricService.getEmittedMetrics().get(1);
        assertTrue(immediateEmitted.immediate);
    }
    
    @Test
    void testServiceOverride() {
        // Create a custom mock service
        MockConfigurationService customMockConfig = new MockConfigurationService();
        customMockConfig.setThingName("custom-thing");
        
        // Override the service
        ggCommons.registerService(IConfigurationService.class, customMockConfig);
        
        // Verify the override worked
        IConfigurationService retrievedService = ggCommons.getService(IConfigurationService.class);
        assertSame(customMockConfig, retrievedService);
        assertEquals("custom-thing", retrievedService.getThingName());
    }
    
    @Test
    void testMultipleInstanceConfiguration() {
        // Test the real ConfigurationService using the actual ConfigManager
        IConfigurationService realConfigService = ggCommons.getService(IConfigurationService.class);
        
        // The real service should have loaded from config_2.json
        // Test that we can access configuration through the real service
        JsonObject globalConfig = realConfigService.getGlobalConfig();
        assertNotNull(globalConfig);
        
        // Test instance access through real service
        Collection<String> instanceIds = realConfigService.getInstanceIds();
        assertNotNull(instanceIds);
        
        // Test that we can get full config
        JsonObject fullConfig = realConfigService.getFullConfig();
        assertNotNull(fullConfig);
        
        // Test template resolution with real service
        String resolved = realConfigService.resolveTemplate("Component: {ComponentName}, Thing: {ThingName}");
        assertNotNull(resolved);
        assertTrue(resolved.contains("Component:"));
        assertTrue(resolved.contains("Thing:"));
        
        // Test thing name and component name access
        assertNotNull(realConfigService.getThingName());
        assertNotNull(realConfigService.getComponentName());
        assertNotNull(realConfigService.getComponentFullName());
    }
    
    @Test
    void testIoTCoreMessaging() {
        String topic = "iot/test/topic";
        JsonObject payload = new JsonObject();
        payload.addProperty("iot", "message");
        
        Message message = Message.buildFromConfig("IoTTest", "1.0", payload, ggCommons.getConfigManager());
        
        // Test IoT Core publish
        mockMessagingService.publishToIotCore(topic, message, QOS.AT_LEAST_ONCE);
        
        assertEquals(1, mockMessagingService.getPublishedMessages().size());
        MockMessagingService.PublishedMessage published = mockMessagingService.getPublishedMessages().get(0);
        assertEquals(topic, published.topic);
        assertEquals(message, published.message);
        assertEquals(QOS.AT_LEAST_ONCE, published.qos);
        
        // Test IoT Core subscription
        TestMessageHandler handler = new TestMessageHandler();
        mockMessagingService.subscribeToIoTCore(topic, handler::handle, QOS.AT_MOST_ONCE);
        
        mockMessagingService.simulateMessage(topic, message);
        assertTrue(handler.wasHandlerCalled());
    }
    
    @Test
    void testMessageConstruction() {
        JsonObject payload = new JsonObject();
        payload.addProperty("testData", "testValue");
        payload.addProperty("number", 42);
        
        Message message = Message.buildFromConfig("TestMessage", "2.1", payload, ggCommons.getConfigManager());
        
        assertNotNull(message);
        assertNotNull(message.getHeader());
        assertNotNull(message.getBody());
        assertNull(message.getRaw());
        
        assertEquals("TestMessage", message.getHeader().getName());
        assertEquals("2.1", message.getHeader().getVersion());
        assertNotNull(message.getHeader().getCorrelationId());
        assertNotNull(message.getHeader().getTimestamp());
        assertFalse(message.getHeader().getTimestamp().isEmpty());
        
        assertEquals(payload, message.getBody());
    }
    
    @Test
    void testMessageHeaders() {
        JsonObject payload = new JsonObject();
        payload.addProperty("data", "value");
        
        Message message1 = Message.buildFromConfig("Msg1", "1.0", payload, ggCommons.getConfigManager());
        Message message2 = Message.buildFromConfig("Msg2", "1.0", payload, ggCommons.getConfigManager());
        
        assertNotEquals(message1.getCorrelationId(), message2.getCorrelationId());
        
        // Parse ISO 8601 timestamp string for comparison
        try {
            Instant timestamp1 = Instant.parse(message1.getHeader().getTimestamp());
            Instant now = Instant.now();
            assertTrue(timestamp1.isBefore(now.plusSeconds(1))); // Allow 1 second tolerance
            assertTrue(timestamp1.isAfter(now.minusSeconds(5))); // Within 5 seconds
        } catch (DateTimeParseException e) {
            fail("Timestamp should be valid ISO 8601 format: " + message1.getHeader().getTimestamp());
        }
        
        // Test that message tags exist
        assertNotNull(message1.getTags());
        assertNotNull(message1.getHeader());
    }
    
    @Test
    void testMetricConstruction() {
        Metric metric1 = new Metric("test_metric");
        assertEquals("test_metric", metric1.getName());
        assertNotNull(metric1.getNamespace());
        
        // Test metric with custom namespace, measures, and dimensions
        Map<String, Measure> measures = new HashMap<>();
        measures.put("response_time", new Measure("response_time", "Milliseconds", 1));
        measures.put("error_count", new Measure("error_count", "Count", 60));
        
        Map<String, String> dimensions = new HashMap<>();
        dimensions.put("Service", "TestService");
        dimensions.put("Region", "us-west-2");
        
        Metric metric2 = new Metric("custom_metric", "CustomNamespace", measures, dimensions);
        assertEquals("custom_metric", metric2.getName());
        assertEquals("CustomNamespace", metric2.getNamespace());
        
        // Verify measures were added
        assertEquals(2, metric2.getMeasures().size());
        assertNotNull(metric2.getMeasure("response_time"));
        assertNotNull(metric2.getMeasure("error_count"));
        assertEquals("Milliseconds", metric2.getMeasure("response_time").getUnit());
        assertEquals("Count", metric2.getMeasure("error_count").getUnit());
        
        // Verify dimensions were added (note: constructor adds default dimensions too)
        assertTrue(metric2.getDimensions().containsKey("Service"));
        assertTrue(metric2.getDimensions().containsKey("Region"));
        assertEquals("TestService", metric2.getDimensions().get("Service"));
        assertEquals("us-west-2", metric2.getDimensions().get("Region"));
        
        Measure measure1 = new Measure("count", "Count", 1);
        Measure measure2 = new Measure("duration", "Milliseconds", 60);
        
        metric1.addMeasure(measure1);
        metric1.addMeasure(measure2);
        
        assertEquals(2, metric1.getMeasures().size());
        assertEquals(measure1, metric1.getMeasure("count"));
        assertEquals(measure2, metric1.getMeasure("duration"));
        
        metric1.addDimension("Environment", "Test");
        assertEquals("Test", metric1.getDimensions().get("Environment"));
    }
    
    @Test
    void testMetricConstructionNegativeTests() {
        // Test metric with null measures (should throw IllegalArgumentException)
        assertThrows(IllegalArgumentException.class, () -> {
            new Metric("null_test_metric", "TestNamespace", null, null);
        });
        
        // Test metric with null dimensions but valid measures (should handle gracefully)
        Map<String, Measure> validMeasures = new HashMap<>();
        validMeasures.put("test_measure", new Measure("test_measure", "Count", 1));
        Metric metric3 = new Metric("null_dimensions_metric", "TestNamespace", validMeasures, null);
        assertEquals("null_dimensions_metric", metric3.getName());
        assertEquals("TestNamespace", metric3.getNamespace());
        assertTrue(metric3.getDimensions().size() >= 3); // Should have at least 3 default dimensions
        
        // Test metric with empty maps
        Map<String, Measure> emptyMeasures = new HashMap<>();
        Map<String, String> emptyDimensions = new HashMap<>();
        Metric metric4 = new Metric("empty_test_metric", "TestNamespace", emptyMeasures, emptyDimensions);
        assertEquals("empty_test_metric", metric4.getName());
        assertEquals("TestNamespace", metric4.getNamespace());
        assertEquals(0, emptyMeasures.size()); // Should remain empty
        assertTrue(metric4.getDimensions().size() >= 3); // Should have at least 3 default dimensions
    }
    
    @Test
    void testTemplateResolution() {
        mockConfigService.setThingName("test-device");
        mockConfigService.setComponentName("TestComponent");
        
        String template = "Device: {ThingName}, Component: {ComponentName}";
        String resolved = mockConfigService.resolveTemplate(template);
        assertEquals("Device: test-device, Component: TestComponent", resolved);
        
        String noVars = mockConfigService.resolveTemplate("No variables");
        assertEquals("No variables", noVars);
    }
    
    @Test
    void testServiceRegistryEdgeCases() {
        assertNull(ggCommons.getService(String.class));
        
        assertThrows(IllegalArgumentException.class, () -> {
            ggCommons.getServiceRegistry().register(IConfigurationService.class, null);
        });
        
        assertThrows(IllegalArgumentException.class, () -> {
            ggCommons.getServiceRegistry().register(null, mockConfigService);
        });
    }
    
    @Test
    void testRawMessageHandling() {
        JsonObject rawPayload = new JsonObject();
        rawPayload.addProperty("raw", "data");
        
        mockMessagingService.publishRaw("test/raw", rawPayload);
        
        assertEquals(1, mockMessagingService.getPublishedMessages().size());
        MockMessagingService.PublishedMessage published = mockMessagingService.getPublishedMessages().get(0);
        assertEquals("test/raw", published.topic);
        assertEquals(rawPayload, published.rawPayload);
        assertNull(published.message);
    }
    
    @Test
    void testMessagingReply() {
        JsonObject originalPayload = new JsonObject();
        originalPayload.addProperty("request", "data");
        Message originalMessage = Message.buildFromConfig("OriginalMsg", "1.0", originalPayload, ggCommons.getConfigManager());
        
        JsonObject replyPayload = new JsonObject();
        replyPayload.addProperty("response", "data");
        Message replyMessage = Message.buildFromConfig("ReplyMsg", "1.0", replyPayload, ggCommons.getConfigManager());
        
        mockMessagingService.reply(originalMessage, replyMessage);
        
        assertEquals(1, mockMessagingService.getPublishedMessages().size());
        MockMessagingService.PublishedMessage published = mockMessagingService.getPublishedMessages().get(0);
        assertEquals("reply", published.topic);
        assertEquals(replyMessage, published.message);
    }
    
    // Helper classes for testing
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
    
    private static class TestMessageHandler {
        private boolean handlerCalled = false;
        private String receivedTopic;
        private Message receivedMessage;
        
        public void handle(String topic, Message message) {
            handlerCalled = true;
            receivedTopic = topic;
            receivedMessage = message;
        }
        
        public boolean wasHandlerCalled() {
            return handlerCalled;
        }
        
        public String getReceivedTopic() {
            return receivedTopic;
        }
        
        public Message getReceivedMessage() {
            return receivedMessage;
        }
    }
}