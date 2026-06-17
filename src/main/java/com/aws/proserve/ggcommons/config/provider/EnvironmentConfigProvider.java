/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.config.provider;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.google.gson.JsonObject;
import com.google.gson.JsonSyntaxException;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

class EnvironmentConfigProvider extends ConfigProvider
{
    protected static final Logger LOGGER = LogManager.getLogger(EnvironmentConfigProvider.class);
    private final String environmentVariableName;

    EnvironmentConfigProvider(ConfigManager configManager, String environmentVariableName)
    {
        super(configManager);
        this.environmentVariableName = environmentVariableName;

    }

    @Override
   public  JsonObject loadConfiguration()
    {
        JsonObject retVal = null;

        LOGGER.debug("Loading configuration from environment variable '{}'", environmentVariableName);
        String configStr = System.getenv(environmentVariableName);
        if (configStr != null)
        {
            try{
                retVal = gson.fromJson(configStr, JsonObject.class);

            }   catch(JsonSyntaxException e)
            {
                LOGGER.fatal("Error parsing configuration: {}\n{}", configStr, e.toString(), e);
                throw new RuntimeException("Error parsing configuration from environment variable '" + environmentVariableName + "'", e);
            }
        }
        else
        {
            LOGGER.fatal("Configuration environment variable ('{}') not defined.", environmentVariableName);
            throw new RuntimeException("Configuration environment variable ('" + environmentVariableName + "') not defined.");
        }

        return retVal;
    }

    @Override
    public String getConfigSource()
    {
        return String.format("Environment (var name: %s)", environmentVariableName);
    }
}
