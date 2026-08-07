/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config.provider;

import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.aws.greengrass.GreengrassCoreIPCClientV2;
import software.amazon.awssdk.aws.greengrass.model.GetConfigurationRequest;
import software.amazon.awssdk.aws.greengrass.model.GetConfigurationResponse;

import java.util.Map;

/**
 * Loads the component configuration from the Greengrass Nucleus over IPC.
 *
 * <p>The Nucleus configuration store keeps JSON numbers as doubles, so a configured
 * {@code intervalSecs: 30} arrives in the IPC response map as the Java {@code Double} {@code 30.0}
 * and lands in this provider's Gson tree as {@code 30.0}. That is repaired once, for every config
 * source, at configuration intake — see
 * {@link com.mbreissi.edgecommons.config.ConfigNumbers} and
 * {@link com.mbreissi.edgecommons.config.ConfigManager} — so nothing here needs to normalize it and
 * the document is passed on exactly as the Nucleus delivered it.
 */
public final class GreengrassConfigProvider extends ConfigProvider
{
    private static final Logger LOGGER = LogManager.getLogger(GreengrassConfigProvider.class);

    final String configComponentName;
    final String configKey;

    private final MessagingClient messagingClient;

    GreengrassConfigProvider(ConfigManager configManager, String configComponentName, String configKey, MessagingClient messagingClient)
    {
        super(configManager);
        this.messagingClient = messagingClient;
        this.configComponentName = configComponentName;
        this.configKey = (configKey == null) ? "ComponentConfig" : configKey;
    }

    @Override
    public JsonObject loadConfiguration()
    {
        JsonObject retVal = new JsonObject();
        LOGGER.debug("Loading Greengrass component configuration");

        GreengrassCoreIPCClientV2 ipcClient = (GreengrassCoreIPCClientV2) messagingClient.getNativeLocalClient();
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

                String tempConfig = gson.toJson(response.getValue());
                JsonObject fullConfig = gson.fromJson(tempConfig, JsonObject.class);
                LOGGER.info("Full configuration retrieved from Nucleus: {}", tempConfig);
                retVal = fullConfig.getAsJsonObject(configKey);
                LOGGER.info("Component configuration retrieved from Nucleus: {}", retVal);
            } else {
                LOGGER.fatal("Configuration not found.  Incorrect component name?");
                throw new RuntimeException("Configuration not found. Incorrect component name?");
            }
        }
        catch (InterruptedException e) // import java.lang.InterruptedException
        {
            Thread.currentThread().interrupt();
            LOGGER.fatal("Thread interrupted while loading configuration.", e);
            throw new RuntimeException("Thread interrupted while loading configuration.", e);
        }

        return retVal;
    }

    @Override
     public String getConfigSource()
    {
        return String.format("Greengrass com.mbreissi.edgecommons.config (component: %s; key: %s)", configComponentName, configKey);
    }
}
