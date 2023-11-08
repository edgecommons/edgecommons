package com.aws.proseve.ggcommons.config.manager;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

public class ConfigManagerBuilder
{
    protected static final Logger LOGGER = LogManager.getLogger(ConfigManagerBuilder.class);

    public static ConfigManager build(String componentName, String[] configArgs)
    {
        ConfigManager configManager = null;
        switch (configArgs[0].toUpperCase())
        {
            case "FILE":
                LOGGER.debug("Using File com.aws.proseve.ggcommons.config manager");
                String configFile = configArgs.length > 1 ? configArgs[1] : "com.aws.proseve.ggcommons.config.json";
                configManager = new FileConfigManager(componentName, configFile);
                break;
            case "ENV":
                LOGGER.debug("Using Environment com.aws.proseve.ggcommons.config manager");
                String envVarName =  configArgs.length > 1 ? configArgs[1] : "CONFIG";
                configManager = new EnvironmentConfigManager(componentName, envVarName);
                break;
            case "SHADOW":
                LOGGER.debug("Using Shadow com.aws.proseve.ggcommons.config manager");
                String shadowName = configArgs.length > 1 ? configArgs[1] : componentName;
                configManager = new ShadowConfigManager(componentName, shadowName);
                break;
            case "GG_CONFIG":
                LOGGER.debug("Using Greengrass com.aws.proseve.ggcommons.config manager");
                String configComponentName = configArgs.length > 1 ? configArgs[1] : null;
                String configKey = configArgs.length > 2 ? configArgs[2] : null;
                configManager = new GreengrassConfigManager(componentName, configComponentName, configKey);
                break;
            default:
                LOGGER.fatal("Unrecognized com.aws.proseve.ggcommons.config source '{}'.  Valid values are 'FILE', 'ENV', 'SHADOW' and 'GG_CONFIG", configArgs[0]);
                System.exit(1);
        }
        LOGGER.info("Configuration loaded from {}", configManager.getConfigSource());
        return configManager;
    }
}
