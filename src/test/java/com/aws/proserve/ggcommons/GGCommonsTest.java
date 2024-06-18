package com.aws.proserve.ggcommons;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.aws.proserve.ggcommons.metrics.Measure;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.aws.proserve.ggcommons.metrics.MetricEmitter;
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
    ConfigManager configManager;
    Message receivedMessage;
    Logger LOGGER;
    Gson gson = new Gson();

    GGCommonsTest()
    {
        String[] args = {
                "-t", "ggcommons-test-2",
                "-m", "MQTT", "a3bgkcole5zuv-ats.iot.us-east-1.amazonaws.com", "8883", "./creds",
                "-c", "FILE", "./config_3.json"
        };
        ggCommons = new GGCommons("com.aws.proserve.greengrass.UnitTests", args);
        configManager = ggCommons.getConfigManager();
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
        Message reply = Message.buildFromConfig("ReplyTest", "1.0", replyPayload, configManager);
        MessagingClient.reply(message, reply);
    }

    public void iotCoreRequestHandler(String topic, Message message)
    {
        JsonObject replyPayload = new JsonObject();
        replyPayload.addProperty("reply_message", "(IoT Core) I have received your request and have replied with this message");
        Message reply = Message.buildFromConfig("ReplyTest", "1.0", replyPayload, configManager);
        MessagingClient.reply(message, reply);
    }

//    @Test
//    void publishIpcMessage()
//    {
//        String topic = "test/testIpcTopic";
//        MessagingClient.subscribe(topic, this::ipcMessageHandler, 1);
//        JsonObject jsonPayload = new JsonObject();
//        jsonPayload.addProperty("message", "Test IPC message");
//        Message msg = Message.buildFromConfig("IpcMessageTest", "1.0", jsonPayload, configManager);
//        MessagingClient.publish(topic, msg);
//        Utils.sleep(200);
//        assertNotNull(receivedMessage);
//        assertEquals(receivedMessage.getHeader().getName(), "IpcMessageTest");
//    }
//
//    @Test
//    void publishIotCoreMessage()
//    {
//        String topic = "test/testIotCoreTopic";
//        MessagingClient.subscribeToIoTCore(topic, this::iotCoreMessageHandler, QOS.AT_LEAST_ONCE);
//        JsonObject jsonPayload = new JsonObject();
//        jsonPayload.addProperty("message", "Test IoT Core message");
//        Message msg = Message.buildFromConfig("IoTCoreMessage", "1.0", jsonPayload, configManager);
//        MessagingClient.publishToIotCore(topic, msg, QOS.AT_LEAST_ONCE);
//        Utils.sleep(200);
//        assertNotNull(receivedMessage);
//        assertEquals(receivedMessage.getHeader().getName(), "IoTCoreMessage");
//    }
//
//    @Test
//    void subscribeWithFilter()
//    {
//        String subTopic = "test/+";
//        String pubTopic = "test/testIpcTopic";
//        MessagingClient.subscribe(subTopic, this::ipcMessageHandler, 1);
//        JsonObject jsonPayload = new JsonObject();
//        jsonPayload.addProperty("message", "Test IPC message");
//        Message msg = Message.buildFromConfig("SubscribeWithFilterTest", "1.0", jsonPayload, configManager);
//        MessagingClient.publish(pubTopic, msg);
//        Utils.sleep(200);
//        assertNotNull(receivedMessage);
//        assertEquals(receivedMessage.getHeader().getName(), "SubscribeWithFilterTest");
//    }
//
//    @Test
//    void requestReplyIpc() throws ExecutionException, InterruptedException, TimeoutException
//    {
//        String requestTopic = "test/request";
//        MessagingClient.subscribe(requestTopic, this::requestHandler, 1);
//        JsonObject requestPayload = new JsonObject();
//        requestPayload.addProperty("message", "Test Request Reply");
//        Message request = Message.buildFromConfig("RequestTest", "1.0", requestPayload, configManager);
//        String correlationId = request.getCorrelationId();
//        Message reply = MessagingClient.request(requestTopic, request).get(1000, TimeUnit.MILLISECONDS);
//        assertNotNull(reply);
//        assertEquals(reply.getCorrelationId(), correlationId);
//        assertEquals(reply.getHeader().getName(), "ReplyTest");
//    }
//
//    @Test
//    void requestReplyIoTCore() throws ExecutionException, InterruptedException, TimeoutException
//    {
//        String requestTopic = "test/iot_core_request";
//        MessagingClient.subscribeToIoTCore(requestTopic, this::iotCoreRequestHandler, QOS.AT_MOST_ONCE, 1);
//        JsonObject requestPayload = new JsonObject();
//        requestPayload.addProperty("message", "Test Request Reply");
//        Message request = Message.buildFromConfig("RequestTest", "1.0", requestPayload, configManager);
//        String correlationId = request.getCorrelationId();
//        Message reply = MessagingClient.requestFromIoTCore(requestTopic, request).get(1000, TimeUnit.MILLISECONDS);
//        assertNotNull(reply);
//        assertEquals(reply.getCorrelationId(), correlationId);
//        assertEquals(reply.getHeader().getName(), "ReplyTest");
//    }
//
//    @Test
//    void emitMetric() throws ExecutionException, InterruptedException, TimeoutException
//    {
//        // Create a Metric named "test" using default namespace and dimensions
//        Metric metric = new Metric("test");
//
//        // Add a measure
//        Measure measure = new Measure("val", "Count", 1);
//        metric.addMeasure(measure);
//
//        // Define the metric
//        MetricEmitter.defineMetric(metric);
//
//        for (int i = 1; i <= 5; i++)
//        {
//            Map<String, Float> measureValues = Map.of("val", (float) i);
//            MetricEmitter.emitMetric("test", measureValues);
//            Utils.sleep(1000);
//        }
//    }
//
//        configManager.applyConfig(loadConfiguration("config_2.json"));
//
//        for (int i = 1; i <= 5; i++)
//        {
//            Map<String, Float> measureValues = Map.of("val", (float) i);
//            MetricEmitter.emitMetric("test", measureValues);
//            LOGGER.trace("This is a trace log message ({})", i);
//            LOGGER.debug("This is a debug log message ({})", i);
//            LOGGER.info("This is an info log message ({})", i);
//            LOGGER.warn("This is a warn log message ({})", i);
//            LOGGER.error("This is an error log message ({})", i);
//            Utils.sleep(1000);
//        }
//    }

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
//        MetricEmitter.defineMetric(metric);
//
//        for (int i = 1; i <= 60; i++)
//        {
//            Map<String, Float> measureValues = Map.of("val", (float) i);
//            MetricEmitter.emitMetric("test", measureValues);
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

}
