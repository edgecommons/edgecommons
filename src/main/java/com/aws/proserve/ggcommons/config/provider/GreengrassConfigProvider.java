package com.aws.proserve.ggcommons.config.provider;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.github.cliftonlabs.json_simple.JsonObject;
import com.github.cliftonlabs.json_simple.Jsoner;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.aws.greengrass.GreengrassCoreIPCClientV2;
import software.amazon.awssdk.aws.greengrass.model.GetConfigurationRequest;
import software.amazon.awssdk.aws.greengrass.model.GetConfigurationResponse;

import java.util.Map;

public class GreengrassConfigProvider extends ConfigProvider
{
    private static final Logger LOGGER = LogManager.getLogger(GreengrassConfigProvider.class);

    final String configComponentName;
    final String configKey;

    GreengrassConfigProvider(ConfigManager configManager, String configComponentName, String configKey)
    {
        super(configManager);
        this.configComponentName = configComponentName;
        this.configKey = (configKey == null) ? "ComponentConfig" : configKey;
    }

    @Override
    public JsonObject loadConfiguration()
    {
        JsonObject retVal = null;
        LOGGER.debug("Loading Greengrass component configuration");

        GreengrassCoreIPCClientV2 ipcClient = (GreengrassCoreIPCClientV2) MessagingClient.getNativeClient();
        try {
            GetConfigurationRequest request;
            if (configComponentName == null) {
                request = new GetConfigurationRequest();
            } else
            {
                request = new GetConfigurationRequest().withComponentName(configComponentName);
            }
            GetConfigurationResponse response = ipcClient.getConfiguration(request);
            Map<String,Object> responseValue = response.getValue();
            if (responseValue != null)
            {
                JsonObject tempConfig = new JsonObject(response.getValue());
                JsonObject fullConfig = (JsonObject) Jsoner.deserialize(tempConfig.toJson());
                LOGGER.info("Full configuration retrieved from Nucleus: {}", fullConfig.toJson());
                retVal = (JsonObject) fullConfig.get(configKey);
                LOGGER.info("Component configuration retrieved from Nucleus: {}", retVal.toJson());
            } else {
                LOGGER.fatal("Configuration not found.  Incorrect component name?  Exiting");
                System.exit(5);
            }
        }
        catch (Exception e)
        {
            LOGGER.fatal("Unable to load configuration using Greengrass IPC.  Exiting.");
            System.exit(1);
        }

        return retVal;
    }

    @Override
     public String getConfigSource()
    {
        return String.format("Greengrass com.aws.proseve.ggcommons.config (component: %s; key: %s)", configComponentName, configKey);
    }
}
