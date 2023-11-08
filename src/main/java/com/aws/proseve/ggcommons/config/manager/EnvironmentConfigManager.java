package com.aws.proseve.ggcommons.config.manager;

import com.github.cliftonlabs.json_simple.JsonException;
import com.github.cliftonlabs.json_simple.JsonObject;
import com.github.cliftonlabs.json_simple.Jsoner;

class EnvironmentConfigManager extends ConfigManager
{
    private final String environmentVariableName;

    EnvironmentConfigManager(String componentName, String environmentVariableName)
    {
        super(componentName);
        this.environmentVariableName = environmentVariableName;
        init();
    }

    @Override
    protected JsonObject loadConfiguration()
    {
        JsonObject retVal = null;

        LOGGER.debug("Loading configuration from environment variable '{}'", environmentVariableName);
        String configStr = System.getenv(environmentVariableName);
        if (configStr != null)
        {
            try
            {
                retVal = (JsonObject) Jsoner.deserialize(configStr);
            }
            catch (JsonException e)
            {
                LOGGER.fatal("Error parsing configuration: {}\n{}", configStr, e.toString());
                System.exit(1);
            }
        }
        else
        {
            LOGGER.fatal("Configuration environment variable ('{}') not defined.", environmentVariableName);
            System.exit(2);
        }

        return retVal;
    }

    @Override
    protected String getConfigSource()
    {
        return String.format("Environment (var name: %s)", environmentVariableName);
    }
}
