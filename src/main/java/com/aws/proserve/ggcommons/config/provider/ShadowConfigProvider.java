package com.aws.proserve.ggcommons.config.provider;


import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.aws.proserve.ggcommons.utils.Utils;
import com.google.gson.Gson;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.aws.greengrass.GreengrassCoreIPCClientV2;
import software.amazon.awssdk.aws.greengrass.SubscribeToTopicResponseHandler;
import software.amazon.awssdk.aws.greengrass.model.*;
import software.amazon.awssdk.eventstreamrpc.StreamResponseHandler;

import java.nio.charset.StandardCharsets;

class ShadowConfigProvider extends ConfigProvider implements  StreamResponseHandler<SubscriptionResponseMessage>
{
    private static final Logger LOGGER = LogManager.getLogger(ShadowConfigProvider.class);
    protected static final String SHADOW_TOPIC_TEMPLATE = "$aws/things/%s/shadow/name/%s/";
    protected static final String ALL_SHADOW_TOPIC_TEMPLATE = "$aws/things/%s/shadow/name/%s/+/+";
    protected final String shadowTopicPrefix;
    private final String shadowName;
    private final String thingName;
    GreengrassCoreIPCClientV2 ipcClient;

    ShadowConfigProvider(ConfigManager configManager, String thingName, String shadowName)
    {
        super(configManager);
        this.shadowName = shadowName;
        this.thingName =thingName;
        this.shadowTopicPrefix = String.format(SHADOW_TOPIC_TEMPLATE, thingName, shadowName);
        connectToIPC();
        subscribeShadowTopics();
    }

    @Override
    public JsonObject loadConfiguration()
    {
        JsonObject retVal = null;
        LOGGER.debug("Loading configuration from named shadow ('{}')", shadowName);
        String componentConfigStr = getConfiguration();
        if (componentConfigStr != null)
        {
            LOGGER.debug("Configuration loaded from named shadow ('{}')", shadowName);
            retVal = Utils.destringify(componentConfigStr);
            reportUpdatedConfiguration(componentConfigStr);
        }
        return retVal;
    }

    @Override
    public String getConfigSource()
    {
        return String.format("Named shadow (shadow name: '%s')", shadowName);
    }

    private void reportUpdatedConfiguration(String componentConfig)
    {
        JsonObject shadowDoc = new JsonObject();
        JsonObject stateDoc = new JsonObject();
        JsonObject reportedDoc = new JsonObject();

        LOGGER.trace("Updating com.aws.proseve.ggcommons.config to:{}", componentConfig);
        reportedDoc.addProperty("ComponentConfig", componentConfig);
        stateDoc.add("reported", reportedDoc);
        shadowDoc.add("state", stateDoc);

        try
        {
            UpdateThingShadowRequest updateRequest = new UpdateThingShadowRequest().withThingName(thingName)
                                                                                   .withShadowName(shadowName)
                                                                                   .withPayload(shadowDoc.toString().getBytes(StandardCharsets.UTF_8));
            UpdateThingShadowResponse updateResponse = ipcClient.updateThingShadow(updateRequest);
            LOGGER.trace("Update shadow response: {}", updateResponse.toString());
        }
        catch (InterruptedException e)
        {
            LOGGER.error("Shadow update failed: {}", e.getMessage());
        }
    }

    private void connectToIPC()
    {
        try
        {
            ipcClient = (GreengrassCoreIPCClientV2) MessagingClient.getNativeClient(); // GreengrassCoreIPCClientV2.builder().build();
        }
        catch (Exception e)
        {
            LOGGER.fatal("Unable to connect to Greengrass IPC.");
            System.exit(5);
        }
    }

    private String getConfiguration()
    {
        String retVal = null;
        try
        {
            GetThingShadowRequest request = new GetThingShadowRequest().withThingName(thingName)
                                                                       .withShadowName(shadowName);
            GetThingShadowResponse response = ipcClient.getThingShadow(request);
            LOGGER.trace("Get shadow response: {}", response.toString());
            byte[] payload = response.getPayload();
            if (payload != null && payload.length > 0)
            {
                String payloadAsString = new String(payload, StandardCharsets.UTF_8);
                JsonObject shadowDoc = new Gson().fromJson(payloadAsString, JsonObject.class);
                JsonObject desiredDoc =  shadowDoc.getAsJsonObject("state").getAsJsonObject("desired");
                retVal = desiredDoc.get("ComponentConfig").toString();
            }
            else
            {
                LOGGER.info("Named shadow document {} is empty", shadowName);
            }
        }
        catch (Exception e)
        {
            LOGGER.error("Failed to get named shadow document {}: {}", shadowName, e.getMessage());
        }
        return retVal;
    }

    private void subscribeShadowTopics()
    {
        String shadowUpdateDeltaTopic = String.format(ALL_SHADOW_TOPIC_TEMPLATE, thingName, shadowName);
        try
        {
            SubscribeToTopicRequest subRequest = new SubscribeToTopicRequest().withTopic(shadowUpdateDeltaTopic)
                                                                              .withReceiveMode(ReceiveMode.RECEIVE_MESSAGES_FROM_OTHERS);
            GreengrassCoreIPCClientV2.StreamingResponse<SubscribeToTopicResponse,
                    SubscribeToTopicResponseHandler> response =
                    ipcClient.subscribeToTopic(subRequest, this);
            LOGGER.info("Subscribed to IPC messages for shadow updates.");
        }
        catch (Exception e)
        {
            LOGGER.error("Failed to subscribe to IPC messages for shadow updates: {}", e.getMessage());
        }
    }

    @Override
    public void onStreamEvent(SubscriptionResponseMessage subscriptionResponseMessage)
    {
        try {
            BinaryMessage binaryMessage = subscriptionResponseMessage.getBinaryMessage();
            String message = new String(binaryMessage.getMessage(), StandardCharsets.UTF_8);
            String topic = binaryMessage.getContext().getTopic();
            String[] topicParts = topic.split("/");
            String action = topicParts[topicParts.length - 2];
            String result = topicParts[topicParts.length - 1];

            if (action.equals("get") && result.equals("rejected")) {
                LOGGER.warn("Named shadow document {} does not exist.  Creating default configuration.", shadowName);
                reportUpdatedConfiguration(getDefaultConfig().toString());
            } else if (action.equals("update") && result.equals("accepted")) {
                LOGGER.debug("Received update/accepted message.  Attempting to apply changes. message:  {}", message);
                String decodedBinaryPayload = new String(subscriptionResponseMessage.getBinaryMessage().getMessage(), StandardCharsets.UTF_8);
                JsonObject payload = new Gson().fromJson(decodedBinaryPayload, JsonObject.class);
                JsonObject desiredDoc = payload.getAsJsonObject("state").getAsJsonObject("desired");
                if (desiredDoc != null)
                {
                    String componentConfigStr = desiredDoc.get("ComponentConfig").toString();
                    JsonObject componentConfig = Utils.destringify(componentConfigStr);
                    configurationChanged(componentConfig);
                    reportUpdatedConfiguration(componentConfigStr);
                }
            } else if (action.equals("update") && result.equals("delta")) {
                LOGGER.warn("Received update/delta message. {}", message);
            }
            else
            {
                LOGGER.debug("Received new shadow message on topic {}. Ignoring", topic);
            }
        } catch (Exception e) {
            LOGGER.error("Exception occurred while processing subscription response: {}, {}", e.getMessage(), e.getStackTrace());
        }
    }

    private JsonObject getDefaultConfig()
    {
        JsonObject retVal = new JsonObject();
        JsonObject logging = new JsonObject();
        JsonObject heartbeat = new JsonObject();
        JsonObject source = new JsonObject();
        JsonObject component = new JsonObject();
        JsonObject global = new JsonObject();
        JsonArray instances = new JsonArray();

        component.add("global", global);
        component.add("instances", instances);
        retVal.add("logging", logging);
        retVal.add("tags", source);
        retVal.add("heartbeat", heartbeat);
        retVal.add("component", component);
        return retVal;
    }

    @Override
    public boolean onStreamError(Throwable throwable)
    {
        LOGGER.error("Error on IPC stream for subscription to shadow updates: {}", throwable.getMessage());
        return false;
    }

    @Override
    public void onStreamClosed()
    {
        LOGGER.info("IPC stream for subscription to shadow updates closed (unsubscribed)");
    }


    protected void configurationChanged(JsonObject newConfig)
    {
        LOGGER.info("configurationChanged: Applying new com.aws.proseve.ggcommons.config: {}", newConfig);
        parentConfigManager.applyConfig(newConfig);


    }

}
