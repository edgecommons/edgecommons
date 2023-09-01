package com.aws.proserve.javacomponentskeleton;

import com.aws.proserve.ggcommons.config.manager.ConfigManager;
import com.aws.proserve.ggcommons.config.manager.ConfigurationChangeListener;
import com.aws.proserve.ggcommons.messaging.ReplyFuture;
import com.github.cliftonlabs.json_simple.JsonObject;
import com.aws.proserve.ggcommons.GGCommons;
import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.math.BigDecimal;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

import static com.aws.proserve.ggcommons.utils.Utils.sleep;

/**
 * Hello world!
 */
public class App implements ConfigurationChangeListener
{
    private static final Logger LOGGER = LogManager.getLogger(App.class);

    GGCommons ggCommons;
    ConfigManager configManager;
    static final String pubTopic = "ggcommons/test/java/hello_world";
    static final String reqTopic = "ggcommons/test/java/request";

    long publishInterval;

    public static void main(String[] args) {
        new App(args);
    }

    public static void ipcHelloWorldHandler(String topic, Message message)
    {
        LOGGER.info("Received an ipc hello world message on topic {}: {}", topic, ((JsonObject) message.getBody()).get("id"));
    }

    public static void iotCoreHelloWorldHandler(String topic, Message message)
    {
        LOGGER.info("Received an iot core hello world message on topic {}: {}", topic, ((JsonObject) message.getBody()).get("id"));
    }

    public void requestCallback(String topic, Message msg)
    {
        LOGGER.info("Received request message [{}]: {}", topic, ((JsonObject) msg.getBody()).get("id"));
        JsonObject replyPayload = new JsonObject();
        replyPayload.put("reply_message", "I have received your request and have replied with this message");
        int waitTimeSecs =  ((BigDecimal) ((JsonObject) msg.getBody()).get("wait_time")).intValue();
        sleep((long) waitTimeSecs*1000L);
        Message reply = Message.buildFromConfig("ReplyTest", "1.0", replyPayload, configManager);
        LOGGER.info("Publishing reply message {}", ((JsonObject) msg.getBody()).get("id"));
        MessagingClient.reply(msg, reply);
    }

    public ReplyFuture publishRequest(String id, float executionTime)
    {
        LOGGER.info("Publishing request message {}", id);
        JsonObject requestPayload = new JsonObject();
        requestPayload.put("id", id);
        requestPayload.put("wait_time", executionTime);
        Message request = Message.buildFromConfig("RequestTest", "1.0", requestPayload, configManager);
        return MessagingClient.request(reqTopic, request);
    }

    public void waitForReply(String msgInstance, ReplyFuture iou, long timeout)
    {
        LOGGER.info("Waiting for reply for {}", msgInstance);
        try
        {
            Message reply = iou.get(timeout*1000, TimeUnit.MILLISECONDS);
            LOGGER.info("...Received reply for {}: {}", msgInstance, reply.toString());
        }
        catch (InterruptedException | ExecutionException e)
        {
            LOGGER.error("Error waiting for reply for {}: {}", msgInstance, e.getMessage());
        }
        catch (TimeoutException e)
        {
            LOGGER.warn("Reply for {} timed out (took more than {} seconds). Cancelling.", msgInstance, timeout);
            MessagingClient.cancelRequest(iou);
        }
    }

    @Override
    public boolean onConfigurationChanged()
    {
        LOGGER.info("Configuration changed. Applying change.");
        publishInterval = ((BigDecimal) configManager.getGlobalConfig().get("publish_interval")).intValue()*1000L;
        return true;
    }

    public App(String[] args)
    {
        ggCommons = new GGCommons("GGComponentSkeleton", args);
        configManager = ggCommons.getConfigManager();
        configManager.addConfigChangeListener(this);
        publishInterval = ((BigDecimal) configManager.getGlobalConfig().get("publish_interval")).intValue()*1000L;

        MessagingClient.subscribe(pubTopic, App::ipcHelloWorldHandler);
        MessagingClient.subscribeToIoTCore(pubTopic, App::iotCoreHelloWorldHandler, QOS.AT_LEAST_ONCE);
        MessagingClient.subscribe(reqTopic, this::requestCallback);

        ReplyFuture iou1 = publishRequest("iou_1", 0);
        ReplyFuture iou2 = publishRequest("iou_2", 1);
        ReplyFuture iou3 = publishRequest("iou_3", 5);
        waitForReply("iou_1", iou1, 1);
        waitForReply("iou_3", iou3, 3);
        waitForReply("iou_2", iou2, 2);

        int i = 1;
        while (true)
        {
            JsonObject jsonPayload = new JsonObject();
            jsonPayload.put("id", i);
            jsonPayload.put("message", "Hello World Java");

            Message msg = Message.buildFromConfig("test", "1.0", jsonPayload, configManager);
            LOGGER.info("Publishing message {} to ipc", i);
            MessagingClient.publish(pubTopic, msg);
            LOGGER.info("Publishing message {} to iot core", i);
            MessagingClient.publishToIotCore(pubTopic, msg, QOS.AT_LEAST_ONCE);

            i++;
            sleep(publishInterval);
        }
    }
}