package com.aws.proseve.ggcommons.config.manager;

import com.github.cliftonlabs.json_simple.JsonObject;
import com.github.cliftonlabs.json_simple.Jsoner;
import software.amazon.awssdk.aws.greengrass.GreengrassCoreIPCClientV2;
import software.amazon.awssdk.aws.greengrass.model.GetConfigurationRequest;
import software.amazon.awssdk.aws.greengrass.model.GetConfigurationResponse;

import java.util.Map;

public class GreengrassConfigManager extends ConfigManager
{
    String configComponentName;
    String configKey;

    GreengrassConfigManager(String componentName, String configComponentName, String configKey)
    {
        super(componentName);
        this.configComponentName = configComponentName;
        this.configKey = (configKey == null) ? "ComponentConfig" : configKey;
        init();
    }

    @Override
    protected JsonObject loadConfiguration()
    {
        JsonObject retVal = null;
        LOGGER.debug("Loading Greengrass component configuration");

        try (GreengrassCoreIPCClientV2 ipcClient = connectToIPC())
        {
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
            throw new RuntimeException(e);
        }

        return retVal;
    }

    @Override
    protected String getConfigSource()
    {
        return String.format("Greengrass com.aws.proseve.ggcommons.config (component: %s; key: %s)", configComponentName, configKey);
    }

    private GreengrassCoreIPCClientV2 connectToIPC()
    {
        GreengrassCoreIPCClientV2 ipcClient = null;
        try
        {
            ipcClient = GreengrassCoreIPCClientV2.builder().build();
        }
        catch (Exception e)
        {
            LOGGER.fatal("Unable to connect to Greengrass IPC to retrieve configuration.");
            System.exit(5);
        }
        return ipcClient;
    }
}
