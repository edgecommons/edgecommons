package com.aws.proserve.javacomponentskeleton;

import com.aws.proserve.ggcommons.config.manager.ConfigManager;
import com.aws.proserve.ggcommons.config.manager.ConfigurationChangeListener;
import com.aws.proserve.ggcommons.messaging.ReplyFuture;
import com.github.cliftonlabs.json_simple.JsonObject;
import com.aws.proserve.ggcommons.GGCommons;
import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.aws.proserve.ggcommons.utils.Utils;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.math.BigDecimal;
import java.util.concurrent.CompletableFuture;
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

    int publishInterval;

    public static void main(String[] args) {
        new App(args);
    }

    public static void callback(String topic, Message message)
    {
        Object body = message.getBody();
        LOGGER.info("Received message [{}]: {}", topic, message.toString());
    }

    public void requestCallback(String topic, Message request)
    {
        LOGGER.info("Received request message [{}]: {}", topic, request.toString());
        JsonObject replyPayload = new JsonObject();
        replyPayload.put("reply_message", "I have received your request");
        sleep(10000);
        Message reply = Message.buildFromConfig("ReplyTest", "1.0", replyPayload, configManager);
        LOGGER.info("Publishing reply message...");
        MessagingClient.reply(request, reply);
    }

    @Override
    public boolean onConfigurationChanged()
    {
        LOGGER.info("Configuration changed. Applying change.");
        publishInterval = ((BigDecimal) configManager.getGlobalConfig().get("publish_interval")).intValue()*1000;
        return true;
    }

    public App(String[] args)
    {
        ggCommons = new GGCommons("GGComponentSkeleton", args);
        configManager = ggCommons.getConfigManager();
        configManager.addConfigChangeListener(this);

        MessagingClient.subscribe("testjava/message", App::callback);
        MessagingClient.subscribe("test/hello_world", App::callback);
        MessagingClient.subscribe("test/request", this::requestCallback);
        String message = (String) configManager.getGlobalConfig().get("message");
        publishInterval = ((BigDecimal) configManager.getGlobalConfig().get("publish_interval")).intValue()*1000;

        LOGGER.info("Publishing request message...");
        JsonObject requestJson = new JsonObject();
        requestJson.put("req_message", message);
        Message request = Message.buildFromConfig("RequestTest", "1.0", requestJson, configManager);
        ReplyFuture replyFuture = null;
        try
        {
            replyFuture = MessagingClient.request("test/request", request);
            Message reply = replyFuture.get(3000, TimeUnit.MILLISECONDS);
            LOGGER.info("Received reply: {}", reply.toString());
        }
        catch (InterruptedException | ExecutionException e)
        {
            LOGGER.error("Error publishing request message: {}", e.getMessage());
        }
        catch (TimeoutException e)
        {
            LOGGER.warn("Timeout publishing request message.");
            MessagingClient.cancelRequest(replyFuture);
        }

        int i = 1;
        while (true)
        {
            JsonObject jsonPayload = new JsonObject();
            jsonPayload.put("index", i);
            jsonPayload.put("message", message);
            Message msg = Message.buildFromConfig("test", "1.0", jsonPayload, configManager);
            LOGGER.info("Publishing message: {}", msg.toString());
            MessagingClient.publish("testjava/message", msg);

//            Integer intPayload = i;
//            msg = Message.buildFromConfig("test", "1.0", intPayload, configManager);
//            MessagingClient.publish("testjava/message", msg);
//
//            String strPayload = "Hello, I must be going";
//            msg = Message.buildFromConfig("test", "1.0", strPayload, configManager);
//            MessagingClient.publish("testjava/message", msg);
//
//            String strJsonPayload = String.format("{\"index\":%d}", i);
//            msg = Message.buildFromConfig("test", "1.0", strJsonPayload, configManager);
//            MessagingClient.publish("testjava/message", msg);

            i++;
            sleep(publishInterval);
        }
    }
}