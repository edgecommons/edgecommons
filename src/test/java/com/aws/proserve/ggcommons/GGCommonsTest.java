package com.aws.proserve.ggcommons;

import com.aws.proserve.ggcommons.interfaces.IConfigurationService;
import com.aws.proserve.ggcommons.interfaces.IMessagingService;
import com.aws.proserve.ggcommons.interfaces.IMetricService;
import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.metrics.Measure;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.aws.proserve.ggcommons.utils.Utils;
import com.google.gson.Gson;
import com.google.gson.JsonObject;
import com.google.gson.JsonSyntaxException;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import software.amazon.awssdk.aws.greengrass.model.QOS;


import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Paths;
import java.util.Map;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

import static org.junit.jupiter.api.Assertions.*;

class GGCommonsTest
{

    GGCommons ggCommons;
    IConfigurationService configService;
    IMessagingService messagingService;
    IMetricService metricService;
    Message receivedMessage;
    Logger LOGGER;
    Gson gson = new Gson();

    GGCommonsTest()
    {
        String[] args = {
                "-t", "test-thing",
//                "-m", "MQTT", "a3bgkcole5zuv-ats.iot.us-east-1.amazonaws.com", "8883", "./creds",
                "-m", "MQTT", "localhost", "1883",
                "-c", "FILE", "./config_2.json"
        };
        ggCommons = new GGCommons("com.aws.proserve.greengrass.UnitTests", args);
        configService = ggCommons.getService(IConfigurationService.class);
        messagingService = ggCommons.getService(IMessagingService.class);
        metricService = ggCommons.getService(IMetricService.class);
        LOGGER = LogManager.getLogger(GGCommonsTest.class);
    }

    @BeforeEach
    void setUp()
    {
        receivedMessage = null;
    }

    @AfterEach
    void tearDown()
    {
    }

    @Test
    void Dummy()
    {
        assertEquals(1, 1);
    }


    public void ipcMessageHandler(String topic, Message message)
    {
        receivedMessage = message;
    }

    public void iotCoreMessageHandler(String topic, Message message)
    {
        receivedMessage = message;
    }

    public void requestHandler(String topic, Message message)
    {
        JsonObject replyPayload = new JsonObject();
        replyPayload.addProperty("reply_message", "I have received your request and have replied with this message");
        Message reply = Message.buildFromConfig("ReplyTest", "1.0", replyPayload, ggCommons.getConfigManager());
        messagingService.reply(message, reply);
    }

    public void iotCoreRequestHandler(String topic, Message message)
    {
        JsonObject replyPayload = new JsonObject();
        replyPayload.addProperty("reply_message", "(IoT Core) I have received your request and have replied with this message");
        Message reply = Message.buildFromConfig("ReplyTest", "1.0", replyPayload, ggCommons.getConfigManager());
        messagingService.reply(message, reply);
    }

    @Test
    void publishIpcMessage()
    {
        String topic = "test/testIpcTopic";
        messagingService.subscribe(topic, this::ipcMessageHandler, 1);
        JsonObject jsonPayload = new JsonObject();
        jsonPayload.addProperty("message", "Test IPC message");
        Message msg = Message.buildFromConfig("IpcMessageTest", "1.0", jsonPayload, ggCommons.getConfigManager());
        messagingService.publish(topic, msg);
        Utils.sleep(200);
        assertNotNull(receivedMessage);
        assertEquals("IpcMessageTest", receivedMessage.getHeader().getName());
    }
//
    @Test
    void publishRawIpcMessage()
    {
        String topic = "test/testIpcTopic";
        messagingService.subscribe(topic, this::ipcMessageHandler, 1);
        JsonObject jsonPayload = new JsonObject();
        jsonPayload.addProperty("message", "Test IPC message");
        messagingService.publishRaw(topic, jsonPayload);
        Utils.sleep(200);
        assertNotNull(receivedMessage);
        assertNull(receivedMessage.getHeader());
        assertNotNull(receivedMessage.getRaw());
    }
//
    @Test
    void publishMinimalHeaderIpcMessage()
    {
        String topic = "test/testIpcTopic";
        messagingService.subscribe(topic, this::ipcMessageHandler, 1);
        JsonObject message = new JsonObject();
        JsonObject header = new JsonObject();
        header.addProperty("reply_to", "ggcommons/reply");
        message.add("header", header);
        messagingService.publishRaw(topic, message);
        Utils.sleep(200);
        assertNotNull(receivedMessage);
        assertNotNull(receivedMessage.getHeader());
        assertEquals("ggcommons/reply", receivedMessage.getHeader().getReplyTo());
        assertNull(receivedMessage.getRaw());
    }
//
    @Test
    void publishIotCoreMessage()
    {
        String topic = "test/testIotCoreTopic";
        messagingService.subscribeToIoTCore(topic, this::iotCoreMessageHandler, QOS.AT_LEAST_ONCE);
        JsonObject jsonPayload = new JsonObject();
        jsonPayload.addProperty("message", "Test IoT Core message");
        Message msg = Message.buildFromConfig("IoTCoreMessage", "1.0", jsonPayload, ggCommons.getConfigManager());
        messagingService.publishToIotCore(topic, msg, QOS.AT_LEAST_ONCE);
        Utils.sleep(200);
        assertNotNull(receivedMessage);
        assertEquals("IoTCoreMessage", receivedMessage.getHeader().getName());
    }
//
    @Test
    void subscribeWithFilter()
    {
        String subTopic = "test/+";
        String pubTopic = "test/testIpcTopic";
        messagingService.subscribe(subTopic, this::ipcMessageHandler, 1);
        JsonObject jsonPayload = new JsonObject();
        jsonPayload.addProperty("message", "Test IPC message");
        Message msg = Message.buildFromConfig("SubscribeWithFilterTest", "1.0", jsonPayload, ggCommons.getConfigManager());
        messagingService.publish(pubTopic, msg);
        Utils.sleep(200);
        assertNotNull(receivedMessage);
        assertEquals("SubscribeWithFilterTest", receivedMessage.getHeader().getName());
    }
//
    @Test
    void requestReplyIpc() throws ExecutionException, InterruptedException, TimeoutException
    {
        String requestTopic = "test/request";
        messagingService.subscribe(requestTopic, this::requestHandler, 1);
        JsonObject requestPayload = new JsonObject();
        requestPayload.addProperty("message", "Test Request Reply");
        Message request = Message.buildFromConfig("RequestTest", "1.0", requestPayload, ggCommons.getConfigManager());
        String correlationId = request.getCorrelationId();
        Message reply = messagingService.request(requestTopic, request).get(1000, TimeUnit.MILLISECONDS);
        assertNotNull(reply);
        assertEquals(correlationId, reply.getCorrelationId());
        assertEquals("ReplyTest", reply.getHeader().getName());
    }
//
    @Test
    void requestReplyIoTCore() throws ExecutionException, InterruptedException, TimeoutException
    {
        String requestTopic = "test/iot_core_request";
        messagingService.subscribeToIoTCore(requestTopic, this::iotCoreRequestHandler, QOS.AT_MOST_ONCE, 1);
        JsonObject requestPayload = new JsonObject();
        requestPayload.addProperty("message", "Test Request Reply");
        Message request = Message.buildFromConfig("RequestTest", "1.0", requestPayload, ggCommons.getConfigManager());
        String correlationId = request.getCorrelationId();
        Message reply = messagingService.requestFromIoTCore(requestTopic, request).get(1000, TimeUnit.MILLISECONDS);
        assertNotNull(reply);
        assertEquals(correlationId, reply.getCorrelationId());
        assertEquals("ReplyTest", reply.getHeader().getName());
    }
//
    @Test
    void emitMetric() throws ExecutionException, InterruptedException, TimeoutException
    {
        // Create a Metric named "test" using default namespace and dimensions
        Metric metric = new Metric("test");

        // Add a measure
        Measure measure = new Measure("val", "Count", 1);
        metric.addMeasure(measure);

        // Define the metric
        metricService.defineMetric(metric);

        for (int i = 1; i <= 5; i++)
        {
            Map<String, Float> measureValues = Map.of("val", (float) i);
            metricService.emitMetric("test", measureValues);
            Utils.sleep(1000);
        }
    }
//
    @Test
    void configurationChangeListenersNotCalledDuringInitialization()
    {
        // Create a test listener to track if it gets called during initialization
        TestConfigurationChangeListener testListener = new TestConfigurationChangeListener();

        // Create a new GGCommons instance (this triggers initialization)
        String[] args = {
                "-t", "test-thing",
                "-m", "MQTT", "localhost", "1883",
                "-c", "FILE", "./config_2.json"
        };
        GGCommons testGGCommons = new GGCommons("com.aws.proserve.test.InitTest", args);
        IConfigurationService testConfigService = testGGCommons.getService(IConfigurationService.class);

        // Add our test listener after initialization
        testConfigService.addConfigChangeListener(testListener);

        // Verify the listener was not called during initialization
        assertFalse(testListener.wasOnConfigurationChangedCalled(),
                "onConfigurationChanged should not be called during initialization");

        // Now trigger an actual configuration change to verify the listener works
        testConfigService.notifyConfigurationChanged();

        // Verify the listener was called for the actual configuration change
        assertTrue(testListener.wasOnConfigurationChangedCalled(),
                "onConfigurationChanged should be called for actual configuration changes");
    }
//
    // Test helper class to track configuration change calls
    private static class TestConfigurationChangeListener implements com.aws.proserve.ggcommons.config.ConfigurationChangeListener
    {
        private boolean onConfigurationChangedCalled = false;

        @Override
        public boolean onConfigurationChanged()
        {
            onConfigurationChangedCalled = true;
            return true;
        }

        public boolean wasOnConfigurationChangedCalled()
        {
            return onConfigurationChangedCalled;
        }
    }
//
//    @Test
//    void monitorConfigFileForChanges() throws ExecutionException, InterruptedException, TimeoutException
//    {
//        // Create a Metric named "test" using default namespace and dimensions
//        Metric metric = new Metric("test");
//
//        // Add a measure
//        Measure measure = new Measure("val", "Count", 1);
//        metric.addMeasure(measure);
//
//        // Define the metric
//        metricService.defineMetric(metric);
//
//        for (int i = 1; i <= 60; i++)
//        {
//            Map<String, Float> measureValues = Map.of("val", (float) i);
//            metricService.emitMetric("test", measureValues);
//            LOGGER.trace("This is a trace log message ({})", i);
//            LOGGER.debug("This is a debug log message ({})", i);
//            LOGGER.info("This is an info log message ({})", i);
//            LOGGER.warn("This is a warn log message ({})", i);
//            LOGGER.error("This is an error log message ({})", i);
//            Utils.sleep(1000);
//        }
//    }
//
//    public JsonObject loadConfiguration(String configFilePath)
//    {
//        LOGGER.debug("Loading configuration from file '{}'", configFilePath);
//        JsonObject retVal = null;
//        try
//        {
//            File file = new File(configFilePath);
//            byte[] bytes = java.nio.file.Files.readAllBytes(Paths.get(file.getAbsolutePath()));
//            String configurationFileContents = new String(bytes, StandardCharsets.UTF_8);
//            retVal = gson.fromJson(configurationFileContents, JsonObject.class);
//        }
//        catch (JsonSyntaxException | IOException e)
//        {
//            LOGGER.fatal("Error reading configuration file '{}': {}", configFilePath, e.toString());
//            System.exit(1);
//        }
//
//        return retVal;
//    }
////
}
