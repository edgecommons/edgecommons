/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons;

import com.mbreissi.edgecommons.config.ConfigurationChangeListener;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.metrics.Measure;
import com.mbreissi.edgecommons.metrics.Metric;
import com.mbreissi.edgecommons.test.MockConfigurationService;
import com.mbreissi.edgecommons.test.MockMessagingService;
import com.mbreissi.edgecommons.test.MockMetricService;

import com.google.gson.JsonObject;
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
 * Unit tests for EdgeCommons library using mock collaborators.
 * These tests do not require external dependencies like MQTT brokers or Greengrass IPC.
 */
class EdgeCommonsUnitTest {

    private static MockConfigurationService mockConfigService;
    private static MockMessagingService mockMessagingService;
    private static MockMetricService mockMetricService;

    @BeforeAll
    static void setUpClass() {
        mockConfigService = new MockConfigurationService();
        mockMessagingService = new MockMessagingService();
        mockMetricService = new MockMetricService();
    }

    @BeforeEach
    void setUp() {
        if (mockMessagingService != null) mockMessagingService.reset();
        if (mockMetricService != null) mockMetricService.reset();
    }

    @Test
    void testConfigurationService() {
        var testConfig = new JsonObject();
        var component = new JsonObject();
        var global = new JsonObject();
        global.addProperty("testProperty", "testValue");
        component.add("global", global);
        testConfig.add("component", component);

        mockConfigService.setFullConfig(testConfig);

        JsonObject globalConfig = mockConfigService.getGlobalConfig();
        assertNotNull(globalConfig);
        assertEquals("testValue", globalConfig.get("testProperty").getAsString());

        mockConfigService.setThingName("unit-test-thing");
        assertEquals("unit-test-thing", mockConfigService.getThingName());

        String resolved = mockConfigService.resolveTemplate("Hello {ThingName}!");
        assertEquals("Hello unit-test-thing!", resolved);
    }

    @Test
    void testConfigurationChangeListener() {
        var listener = new TestConfigurationChangeListener();

        mockConfigService.addConfigChangeListener(listener);
        assertFalse(listener.wasOnConfigurationChangedCalled());

        mockConfigService.simulateConfigurationChange();
        assertTrue(listener.wasOnConfigurationChangedCalled());

        mockConfigService.removeConfigChangeListener(listener);
        listener.reset();
        mockConfigService.simulateConfigurationChange();
        assertFalse(listener.wasOnConfigurationChangedCalled());
    }

    @Test
    void testMessagingService() {
        String testTopic = "test/topic";
        var testPayload = new JsonObject();
        testPayload.addProperty("message", "test message");

        Message testMessage = MessageBuilder.create("TestMessage", "1.0")
                .withPayload(testPayload)
                .withConfig(mockConfigService)
                .build();
        assertNotNull(testMessage);
        assertEquals("TestMessage", testMessage.getHeader().getName());
        assertEquals("1.0", testMessage.getHeader().getVersion());

        mockMessagingService.publish(testTopic, testMessage);
        assertEquals(1, mockMessagingService.getPublishedMessages().size());

        MockMessagingService.PublishedMessage published = mockMessagingService.getPublishedMessages().get(0);
        assertEquals(testTopic, published.topic);
        assertEquals(testMessage, published.message);

        mockMessagingService.publishRaw(testTopic, testPayload);
        assertEquals(2, mockMessagingService.getPublishedMessages().size());

        MockMessagingService.PublishedMessage rawPublished = mockMessagingService.getPublishedMessages().get(1);
        assertEquals(testTopic, rawPublished.topic);
        assertEquals(testPayload, rawPublished.rawPayload);
    }

    @Test
    void testMessagingSubscription() {
        String testTopic = "test/subscription";
        var handler = new TestMessageHandler();

        mockMessagingService.subscribe(testTopic, handler::handle, 1);

        var payload = new JsonObject();
        payload.addProperty("test", "data");
        Message testMessage = MessageBuilder.create("SubTest", "1.0")
                .withPayload(payload)
                .withConfig(mockConfigService)
                .build();

        mockMessagingService.simulateMessage(testTopic, testMessage);

        assertTrue(handler.wasHandlerCalled());
        assertEquals(testTopic, handler.getReceivedTopic());
        assertEquals(testMessage, handler.getReceivedMessage());
    }

    @Test
    void testMessagingRequestResponse() throws ExecutionException, InterruptedException, TimeoutException {
        String requestTopic = "test/request";
        var requestPayload = new JsonObject();
        requestPayload.addProperty("request", "data");

        Message requestMessage = MessageBuilder.create("RequestTest", "1.0")
                .withPayload(requestPayload)
                .withConfig(mockConfigService)
                .build();

        CompletableFuture<Message> responseFuture = mockMessagingService.request(requestTopic, requestMessage);
        Message response = responseFuture.get(1000, TimeUnit.MILLISECONDS);

        assertNotNull(response);
        assertEquals(requestMessage.getCorrelationId(), response.getCorrelationId());

        assertEquals(1, mockMessagingService.getPublishedMessages().size());
        MockMessagingService.PublishedMessage published = mockMessagingService.getPublishedMessages().get(0);
        assertEquals(requestTopic, published.topic);
        assertEquals(requestMessage, published.message);
    }

    @Test
    void testMetricService() {
        var measures = new HashMap<String, Measure>();
        measures.put("count", new Measure("count", "Count", 1));
        measures.put("latency", new Measure("latency", "Milliseconds", 1));
        var testMetric = new Metric("test_metric", "TestNamespace", measures, new HashMap<>());

        mockMetricService.defineMetric(testMetric);

        assertTrue(mockMetricService.isMetricDefined("test_metric"));
        assertEquals(1, mockMetricService.getDefinedMetrics().size());
        assertEquals(testMetric, mockMetricService.getDefinedMetrics().get("test_metric"));

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

        mockMetricService.emitMetricNow("test_metric", values);
        assertEquals(2, mockMetricService.getEmittedMetrics().size());

        MockMetricService.EmittedMetric immediateEmitted = mockMetricService.getEmittedMetrics().get(1);
        assertTrue(immediateEmitted.immediate);
    }

    @Test
    void testConfigAccessViaMock() {
        JsonObject globalConfig = mockConfigService.getGlobalConfig();
        assertNotNull(globalConfig);

        Collection<String> instanceIds = mockConfigService.getInstanceIds();
        assertNotNull(instanceIds);

        JsonObject fullConfig = mockConfigService.getFullConfig();
        assertNotNull(fullConfig);

        String resolved = mockConfigService.resolveTemplate("Component: {ComponentName}, Thing: {ThingName}");
        assertNotNull(resolved);
        assertTrue(resolved.contains("Component:"));
        assertTrue(resolved.contains("Thing:"));

        assertNotNull(mockConfigService.getThingName());
        assertNotNull(mockConfigService.getComponentName());
        assertNotNull(mockConfigService.getComponentFullName());
    }

    @Test
    void testIoTCoreMessaging() {
        String topic = "iot/test/topic";
        var payload = new JsonObject();
        payload.addProperty("iot", "message");

        Message message = MessageBuilder.create("IoTTest", "1.0")
                .withPayload(payload)
                .withConfig(mockConfigService)
                .build();

        mockMessagingService.publishToIoTCore(topic, message, QOS.AT_LEAST_ONCE);

        assertEquals(1, mockMessagingService.getPublishedMessages().size());
        MockMessagingService.PublishedMessage published = mockMessagingService.getPublishedMessages().get(0);
        assertEquals(topic, published.topic);
        assertEquals(message, published.message);
        assertEquals(QOS.AT_LEAST_ONCE, published.qos);

        var handler = new TestMessageHandler();
        mockMessagingService.subscribeToIoTCore(topic, handler::handle, QOS.AT_MOST_ONCE);

        mockMessagingService.simulateMessage(topic, message);
        assertTrue(handler.wasHandlerCalled());
    }

    @Test
    void testMessageConstruction() {
        var payload = new JsonObject();
        payload.addProperty("testData", "testValue");
        payload.addProperty("number", 42);

        Message message = MessageBuilder.create("TestMessage", "2.1")
                .withPayload(payload)
                .withConfig(mockConfigService)
                .build();

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
        var payload = new JsonObject();
        payload.addProperty("data", "value");

        Message message1 = MessageBuilder.create("Msg1", "1.0")
                .withPayload(payload)
                .withConfig(mockConfigService)
                .build();
        Message message2 = MessageBuilder.create("Msg2", "1.0")
                .withPayload(payload)
                .withConfig(mockConfigService)
                .build();

        assertNotEquals(message1.getCorrelationId(), message2.getCorrelationId());

        try {
            Instant timestamp1 = Instant.parse(message1.getHeader().getTimestamp());
            Instant now = Instant.now();
            assertTrue(timestamp1.isBefore(now.plusSeconds(1)));
            assertTrue(timestamp1.isAfter(now.minusSeconds(5)));
        } catch (DateTimeParseException e) {
            fail("Timestamp should be valid ISO 8601 format: " + message1.getHeader().getTimestamp());
        }

        assertNotNull(message1.getTags());
        assertNotNull(message1.getHeader());
    }

    @Test
    void testMetricConstruction() {
        var basicMeasures = new HashMap<String, Measure>();
        basicMeasures.put("basic", new Measure("basic", "Count", 1));
        var metric1 = new Metric("test_metric", "TestNamespace", basicMeasures, new HashMap<>());
        assertEquals("test_metric", metric1.getName());
        assertEquals("TestNamespace", metric1.getNamespace());

        var measures = new HashMap<String, Measure>();
        measures.put("response_time", new Measure("response_time", "Milliseconds", 1));
        measures.put("error_count", new Measure("error_count", "Count", 60));

        var dimensions = new HashMap<String, String>();
        dimensions.put("Service", "TestService");
        dimensions.put("Region", "us-west-2");

        var metric2 = new Metric("custom_metric", "CustomNamespace", measures, dimensions);
        assertEquals("custom_metric", metric2.getName());
        assertEquals("CustomNamespace", metric2.getNamespace());

        assertEquals(2, metric2.getMeasures().size());
        assertNotNull(metric2.getMeasure("response_time"));
        assertNotNull(metric2.getMeasure("error_count"));
        assertEquals("Milliseconds", metric2.getMeasure("response_time").unit());
        assertEquals("Count", metric2.getMeasure("error_count").unit());

        assertTrue(metric2.getDimensions().containsKey("Service"));
        assertTrue(metric2.getDimensions().containsKey("Region"));
        assertEquals("TestService", metric2.getDimensions().get("Service"));
        assertEquals("us-west-2", metric2.getDimensions().get("Region"));

        var measure1 = new Measure("count", "Count", 1);
        var measure2 = new Measure("duration", "Milliseconds", 60);

        metric1.addMeasure(measure1);
        metric1.addMeasure(measure2);

        // metric1 was constructed with "basic" and then had "count" and "duration" added.
        assertEquals(3, metric1.getMeasures().size());
        assertEquals(measure1, metric1.getMeasure("count"));
        assertEquals(measure2, metric1.getMeasure("duration"));

        metric1.addDimension("Environment", "Test");
        assertEquals("Test", metric1.getDimensions().get("Environment"));
    }

    @Test
    void testMetricConstructionNegativeTests() {
        assertThrows(IllegalArgumentException.class, () -> {
            new Metric("null_test_metric", "TestNamespace", null, null);
        });

        var validMeasures = new HashMap<String, Measure>();
        validMeasures.put("test_measure", new Measure("test_measure", "Count", 1));
        var metric3 = new Metric("null_dimensions_metric", "TestNamespace", validMeasures, null);
        assertEquals("null_dimensions_metric", metric3.getName());
        assertEquals("TestNamespace", metric3.getNamespace());
        // The Metric constructor adds the "category" default dimension.
        assertTrue(metric3.getDimensions().containsKey("category"));

        var emptyMeasures = new HashMap<String, Measure>();
        var emptyDimensions = new HashMap<String, String>();
        var metric4 = new Metric("empty_test_metric", "TestNamespace", emptyMeasures, emptyDimensions);
        assertEquals("empty_test_metric", metric4.getName());
        assertEquals("TestNamespace", metric4.getNamespace());
        assertEquals(0, emptyMeasures.size());
        assertTrue(metric4.getDimensions().containsKey("category"));
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
    void testRawMessageHandling() {
        var rawPayload = new JsonObject();
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
        var originalPayload = new JsonObject();
        originalPayload.addProperty("request", "data");
        Message originalMessage = MessageBuilder.create("OriginalMsg", "1.0")
                .withPayload(originalPayload)
                .withConfig(mockConfigService)
                .build();

        var replyPayload = new JsonObject();
        replyPayload.addProperty("response", "data");
        Message replyMessage = MessageBuilder.create("ReplyMsg", "1.0")
                .withPayload(replyPayload)
                .withConfig(mockConfigService)
                .build();

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
