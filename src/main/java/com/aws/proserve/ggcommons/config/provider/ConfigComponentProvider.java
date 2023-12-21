package com.aws.proserve.ggcommons.config.provider;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.aws.proserve.ggcommons.messaging.ReplyFuture;
import com.github.cliftonlabs.json_simple.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

public class ConfigComponentProvider extends ConfigProvider {
    private static final Logger LOGGER = LogManager.getLogger(ConfigComponentProvider.class);
    public static final String GET_TOPIC_TEMPLATE = "ggcommons/{ThingName}/config/get/{ComponentName}";
    public static final String UPDATED_TOPIC_TEMPLATE = "ggcommons/{ThingName}/config/{ComponentName}/updated";

    private final String source;


    ConfigComponentProvider(ConfigManager configManager) {
        super(configManager);
        source=configManager.resolveTemplate(GET_TOPIC_TEMPLATE );
        String updated=configManager.resolveTemplate(UPDATED_TOPIC_TEMPLATE);
        MessagingClient.subscribe(updated,(topic, msg)->{
            parentConfigManager.applyConfig(loadConfiguration());
        });
    }

    @Override
    public JsonObject loadConfiguration() {

        JsonObject requestPayload = new JsonObject();
        Message request = Message.buildFromConfig("GetConfiguration", "1.0", requestPayload, this.parentConfigManager);
        final ReplyFuture replyFuture = MessagingClient.request(source, request);
        Message replyMessage = null;
        int attemptCount = 0;
        boolean retry =true;
        do {
            try {
                replyMessage = replyFuture.get(30, TimeUnit.SECONDS);
                retry = false;
            } catch (InterruptedException e) {
                LOGGER.fatal("Encountered InterruptedException. Unable to load configuration using Greengrass IPC.  Exiting.");
                System.exit(1);
            } catch (ExecutionException e) {
                LOGGER.fatal("Encountered ExecutionException. Unable to load configuration using Greengrass IPC.  Exiting.");
                System.exit(1);
            } catch (TimeoutException e) {
                attemptCount++;
                if (attemptCount ==3) {
                    LOGGER.fatal("Encountered TimeoutException. Unable to load configuration using Greengrass IPC.  Exiting.");
                    System.exit(1);
                }
                LOGGER.warn("Encountered TimeoutException. Unable to load configuration using Greengrass IPC.  Retrying ({})",attemptCount);
            }
        }while(retry) ;
        return (JsonObject) replyMessage.getBody();
    }

    @Override
    public String getConfigSource() {
        return String.format("Config Manager Component (source topic name: %s)", source);
    }

}
