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

    // Suppressing i18n warning as these are internal configuration identifiers
    // that should not be localized
    @SuppressWarnings("i18n")
    public static ConfigProvider build(ConfigManager configManager, String componentName, String thingName, String[] configArgs, com.aws.proserve.ggcommons.messaging.MessagingClient messagingClient) {
        ConfigProvider configProvider = switch (configArgs[0].toUpperCase()) {
            case "FILE" -> {
                LOGGER.debug("Using File com.aws.proseve.ggcommons.config provider");
                String configFile = configArgs.length > 1 ? configArgs[1] : "com.aws.proseve.ggcommons.config.json";
                yield new FileConfigProvider(configManager, configFile);
            }
            case "ENV" -> {
                LOGGER.debug("Using Environment com.aws.proseve.ggcommons.config provider");
                String envVarName = configArgs.length > 1 ? configArgs[1] : "CONFIG";
                yield new EnvironmentConfigProvider(configManager, envVarName);
            }
            case "SHADOW" -> {
                LOGGER.debug("Using Shadow com.aws.proseve.ggcommons.config provider");
                String shadowName = configArgs.length > 1 ? configArgs[1] : componentName;
                if (messagingClient == null) {
                    throw new IllegalStateException("MessagingClient required for SHADOW config provider but not available during initialization");
                }
                yield new ShadowConfigProvider(configManager, thingName, shadowName, messagingClient);
            }
            case "GG_CONFIG" -> {
                LOGGER.debug("Using Greengrass com.aws.proseve.ggcommons.config provider");
                String configComponentName = configArgs.length > 1 ? configArgs[1] : null;
                String configKey = configArgs.length > 2 ? configArgs[2] : null;
                if (messagingClient == null) {
                    throw new IllegalStateException("MessagingClient required for GG_CONFIG config provider but not available during initialization");
                }
                yield new GreengrassConfigProvider(configManager, configComponentName, configKey, messagingClient);
            }
            case "CONFIG_COMPONENT" -> {
                LOGGER.debug("Using Config com.aws.proseve.ggcommons.config provider");
                if (messagingClient == null) {
                    throw new IllegalStateException("MessagingClient required for CONFIG_COMPONENT config provider but not available during initialization");
                }
                yield new ConfigComponentProvider(configManager, messagingClient);
            }
            default -> {
                LOGGER.fatal("Unrecognized config source '{}'.  Valid values are 'FILE', 'ENV', 'SHADOW', 'GG_CONFIG', 'CONFIG_COMPONENT'", configArgs[0]);
                throw new IllegalArgumentException("Unrecognized config source: " + configArgs[0]);
            }
        };
        LOGGER.info("Will load configuration from {}", configProvider.getConfigSource());
        return configProvider;
    }
}
