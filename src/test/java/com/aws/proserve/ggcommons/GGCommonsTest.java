package com.aws.proserve.ggcommons;

import com.aws.proserve.ggcommons.interfaces.IConfigurationService;
import com.aws.proserve.ggcommons.interfaces.IMessagingService;
import com.aws.proserve.ggcommons.interfaces.IMetricService;
import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessageBuilder;
import com.aws.proserve.ggcommons.metrics.Measure;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.aws.proserve.ggcommons.metrics.MetricBuilder;
import com.aws.proserve.ggcommons.utils.Utils;
import com.google.gson.Gson;
import com.google.gson.JsonObject;
import com.google.gson.JsonSyntaxException;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.junit.jupiter.api.*;
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
    private static GGCommons ggCommons;
    private static IConfigurationService configService;
    private static IMessagingService messagingService;
    private static IMetricService metricService;
    private static Logger LOGGER;
    
    private Message receivedMessage;
    private Gson gson = new Gson();

    @BeforeAll
    static void setUpClass()
    {
        String[] args = {
                "-t", "test-thing",
                "-m", "STANDALONE", "./standalone-messaging-sample.json",
                "-c", "FILE", "./config_2.json"
        };
        ggCommons = GGCommonsBuilder.create("com.aws.proserve.greengrass.IntegrationTests")
                .withArgs(args)
                .build();
        configService = ggCommons.getService(IConfigurationService.class);
        messagingService = ggCommons.getService(IMessagingService.class);
        metricService = ggCommons.getService(IMetricService.class);
        LOGGER = LogManager.getLogger(GGCommonsTest.class);
    }
    
    @AfterAll
    static void tearDownClass()
    {
        // Cleanup if needed in future
    }

    @BeforeEach
    void setUp()
    {
        receivedMessage = null;
    }

    @AfterEach
    void tearDown()
    {
        // Clean up subscriptions to prevent interference between tests
        try {
            messagingService.unsubscribe("test/testIpcTopic");
            messagingService.unsubscribe("test/+");
            messagingService.unsubscribe("test/request");
            messagingService.unsubscribeFromIoTCore("test/testIotCoreTopic");
            messagingService.unsubscribeFromIoTCore("test/iot_core_request");
        } catch (Exception e) {
            // Ignore cleanup errors
        }
    }

    @Test
    void Dummy()
    {
        assertEquals(1, 1);
    }


    public void ipcMessageHandler(String topic, Message message)
    {
        LOGGER.info("Received a published message on local messaging system");
        receivedMessage = message;
    }

    public void iotCoreMessageHandler(String topic, Message message)
    {
        LOGGER.info("Received a published message on iot core messaging system");
        receivedMessage = message;
    }

    public void requestHandler(String topic, Message message)
    {
        JsonObject replyPayload = new JsonObject();
        replyPayload.addProperty("reply_message", "I have received your request and have replied with this message");
        Message reply = MessageBuilder.create("ReplyTest", "1.0")
                                      .withPayload(replyPayload).
                                      withConfig(configService)
                                      .build();
        LOGGER.info("Received a request message on local messaging system");
        messagingService.reply(message, reply);
    }

    public void iotCoreRequestHandler(String topic, Message message)
    {
        JsonObject replyPayload = new JsonObject();
        replyPayload.addProperty("reply_message", "(IoT Core) I have received your request and have replied with this message");
        Message reply = MessageBuilder.create("ReplyTest", "1.0")
                .withPayload(replyPayload)
                .withConfig(configService)
                .build();
        messagingService.reply(message, reply);
    }

    @Test
    void publishIpcMessage()
    {
        String topic = "test/testIpcTopic";
        messagingService.subscribe(topic, this::ipcMessageHandler, 1);
        JsonObject jsonPayload = new JsonObject();
        jsonPayload.addProperty("message", "Test IPC message");
        Message msg = MessageBuilder.create("IpcMessageTest", "1.0")
                .withPayload(jsonPayload)
                .withConfig(configService)
                .build();
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
        Message msg = MessageBuilder.create("IoTCoreMessage", "1.0")
                .withPayload(jsonPayload)
                .withConfig(configService)
                .build();
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
        Message msg = MessageBuilder.create("SubscribeWithFilterTest", "1.0")
                .withPayload(jsonPayload)
                .withConfig(configService)
                .build();
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
        Message request = MessageBuilder.create("RequestTest", "1.0")
                .withPayload(requestPayload)
                .withConfig(configService)
                .build();
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
        JsonObject requestPayload = new JsonObject();
        requestPayload.addProperty("message", "Test Request Reply");
        Message request = MessageBuilder.create("RequestTest", "1.0")
                .withPayload(requestPayload)
                .withConfig(configService)
                .build();
        String correlationId = request.getCorrelationId();
        LOGGER.info("Sending request to IoT Core on {}", requestTopic);
        Message reply = messagingService.requestFromIoTCore(requestTopic, request).get(1000, TimeUnit.MILLISECONDS);
        assertNotNull(reply);
        assertEquals(correlationId, reply.getCorrelationId());
    }
//
    @Test
    void emitMetric() throws ExecutionException, InterruptedException, TimeoutException
    {
        // Create a Metric named "test" using default namespace and dimensions
        Metric metric = MetricBuilder.create("test")
                .addMeasure("val", "Count", 1)
                .withNamespace(configService.getMetricConfig().getNamespace())
                .build();

        // Define the metric
        metricService.defineMetric(metric);

        for (int i = 1; i <= 5; i++)
        {
            Map<String, Float> measureValues = Map.of("val", (float) i);
            metricService.emitMetric("test", measureValues);
            Utils.sleep(1000);
        }
    }
    
    @Test
    void dualSubscriptionTest()
    {
        String topic = "test/dualTopic";

        // Subscribe to the same topic on both local and IoT Core
        LOGGER.info("Subscribing to LOCAL messages on {}", topic);
        messagingService.subscribe(topic, (t, m) -> {
            LOGGER.info("Received message on LOCAL: {}", m.getHeader().getName());
        }, 1);

        LOGGER.info("Subscribing to IOT CORE messages on {}", topic);
        messagingService.subscribeToIoTCore(topic, (t, m) -> {
            LOGGER.info("Received message on IOT CORE: {}", m.getHeader().getName());
        }, QOS.AT_LEAST_ONCE, 1);
        
        // Publish to local - should only trigger local callback
        JsonObject localPayload = new JsonObject();
        localPayload.addProperty("source", "local");
        Message localMsg = MessageBuilder.create("LocalMessage", "1.0")
                .withPayload(localPayload)
                .withConfig(configService)
                .build();
        LOGGER.info("Publishing message to LOCAL on topic");
        messagingService.publish(topic, localMsg);
        
        // Publish to IoT Core - should only trigger IoT Core callback
        JsonObject iotPayload = new JsonObject();
        iotPayload.addProperty("source", "iotcore");
        Message iotMsg = MessageBuilder.create("IoTCoreMessage", "1.0")
                .withPayload(iotPayload)
                .withConfig(configService)
                .build();
        LOGGER.info("Publishing message to IOT CORE on topic");
        messagingService.publishToIotCore(topic, iotMsg, QOS.AT_LEAST_ONCE);
        
        Utils.sleep(500);
        
        // Clean up
        messagingService.unsubscribe(topic);
        messagingService.unsubscribeFromIoTCore(topic);
    }
}
