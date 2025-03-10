/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.config.provider;

import com.aws.proserve.ggcommons.config.ConfigManager;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

public class ConfigProviderBuilder {
    private static final Logger LOGGER = LogManager.getLogger(ConfigProviderBuilder.class);

    public static ConfigProvider build(ConfigManager configManager, String componentName, String thingName, String[] configArgs) {
        ConfigProvider configProvider = null;
        switch (configArgs[0].toUpperCase()) {
            case "FILE":
                LOGGER.debug("Using File com.aws.proseve.ggcommons.config provider");
                String configFile = configArgs.length > 1 ? configArgs[1] : "com.aws.proseve.ggcommons.config.json";
                configProvider = new FileConfigProvider(configManager,configFile);
                break;
            case "ENV":
                LOGGER.debug("Using Environment com.aws.proseve.ggcommons.config provider");
                String envVarName = configArgs.length > 1 ? configArgs[1] : "CONFIG";
                configProvider = new EnvironmentConfigProvider(configManager, envVarName);
                break;
            case "SHADOW":
                LOGGER.debug("Using Shadow com.aws.proseve.ggcommons.config provider");
                String shadowName = configArgs.length > 1 ? configArgs[1] : componentName;
                configProvider = new ShadowConfigProvider(configManager, thingName, shadowName);
                break;
            case "GG_CONFIG":
                LOGGER.debug("Using Greengrass com.aws.proseve.ggcommons.config provider");
                String configComponentName = configArgs.length > 1 ? configArgs[1] : null;
                String configKey = configArgs.length > 2 ? configArgs[2] : null;
                configProvider = new GreengrassConfigProvider(configManager, configComponentName, configKey);
                break;
            case "CONFIG_COMPONENT":
                LOGGER.debug("Using Config com.aws.proseve.ggcommons.config provider");
                configProvider =new ConfigComponentProvider(configManager);
                break;
            default:
                LOGGER.fatal("Unrecognized com.aws.proseve.ggcommons.config source '{}'.  Valid values are 'FILE', 'ENV', 'SHADOW' and 'GG_CONFIG", configArgs[0]);
                System.exit(1);
        }
        LOGGER.info("Will load configuration from {}", configProvider.getConfigSource());
        return configProvider;
    }
}
