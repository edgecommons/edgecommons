package com.aws.proserve.ggcommons.config.provider;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.github.cliftonlabs.json_simple.JsonException;
import com.github.cliftonlabs.json_simple.JsonObject;
import com.github.cliftonlabs.json_simple.Jsoner;
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
    public String getConfigSource()
    {
        return String.format("Environment (var name: %s)", environmentVariableName);
    }
}
