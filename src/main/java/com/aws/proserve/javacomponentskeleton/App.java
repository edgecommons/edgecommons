package com.aws.proserve.javacomponentskeleton;

import com.aws.proserve.ggcommons.config.manager.ConfigManager;
import com.aws.proserve.ggcommons.config.manager.ConfigurationChangeListener;
import com.github.cliftonlabs.json_simple.JsonObject;
import com.aws.proserve.ggcommons.GGCommons;
import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.aws.proserve.ggcommons.utils.Utils;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.math.BigDecimal;

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

    @Override
    public boolean onConfigurationChanged()
    {
        // cycle through the clients and shut them down. then recreate them.

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
        String message = (String) configManager.getGlobalConfig().get("message");
        publishInterval = ((BigDecimal) configManager.getGlobalConfig().get("publish_interval")).intValue()*1000;
        int i = 1;
        while (true)
        {
            JsonObject jsonPayload = new JsonObject();
            jsonPayload.put("index", i);
            jsonPayload.put("message", message);
            Message msg = Message.buildFromConfig("test", "1.0", jsonPayload, configManager);
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
            Utils.sleep(publishInterval);
        }
    }
}