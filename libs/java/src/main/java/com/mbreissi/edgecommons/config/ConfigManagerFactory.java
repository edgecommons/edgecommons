/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

import com.mbreissi.edgecommons.ParsedCommandLine;
import com.mbreissi.edgecommons.config.provider.ConfigProvider;
import com.mbreissi.edgecommons.config.provider.ConfigProviderBuilder;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.regex.Pattern;

/**
 * Factory for creating ConfigManager instances with proper validation and error handling.
 */
public class ConfigManagerFactory {
    private static final Logger LOGGER = LogManager.getLogger(ConfigManagerFactory.class);
    private static final Pattern VALIDATOR_NAME =
            Pattern.compile("^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$");
    
    /**
     * Creates a ConfigManager instance for the specified component.
     *
     * @param componentName The name of the Greengrass component
     * @param cmdLine Parsed command line arguments containing configuration options
     * @return Configured ConfigManager instance
     * @throws ConfigurationException if configuration loading or validation fails
     */
    public static ConfigManager create(String componentName, ParsedCommandLine cmdLine) throws ConfigurationException {
        return create(componentName, cmdLine, null);
    }

    /**
     * Creates a ConfigManager, supplying a messaging client for config sources (GG_CONFIG,
     * CONFIG_COMPONENT) that load the component configuration over IPC.
     *
     * @param componentName The name of the Greengrass component
     * @param cmdLine Parsed command line arguments containing configuration options
     * @param messagingClient The messaging client (required for GG_CONFIG / CONFIG_COMPONENT sources;
     *                        may be null for FILE / ENV / SHADOW)
     * @return Configured ConfigManager instance
     * @throws ConfigurationException if configuration loading or validation fails
     */
    public static ConfigManager create(String componentName, ParsedCommandLine cmdLine,
                                       MessagingClient messagingClient) throws ConfigurationException {
        return create(componentName, cmdLine, messagingClient, Map.of(),
                ConfigManager.DEFAULT_CANDIDATE_VALIDATION_TIMEOUT);
    }

    /**
     * Creates a manager with ordered, pre-commit component validators. Provider watchers and
     * subscriptions are started only after schema validation, INITIAL validation, snapshot
     * construction, and manager attachment have all succeeded.
     */
    public static ConfigManager create(String componentName, ParsedCommandLine cmdLine,
                                       MessagingClient messagingClient,
                                       Map<String, ConfigurationCandidateValidator> validators,
                                       Duration validationTimeout) throws ConfigurationException {
        ConfigProvider configProvider = null;
        try {
            // Parse component name
            String componentFullName = componentName;
            String componentShortName = componentName.contains(".")
                ? componentName.substring(componentName.lastIndexOf(".") + 1)
                : componentName;

            // Determine thing name
            String thingName = resolveThingName(cmdLine);

            // Load the provider configuration. Direct providers are already single effective
            // documents; CONFIG_COMPONENT replies are lineage bundles merged into one effective
            // document before validation.
            configProvider = ConfigProviderBuilder.build(null, componentName, thingName,
                    cmdLine.configArgs, messagingClient);
            LayeredConfigCoordinator layeredConfigCoordinator =
                    new LayeredConfigCoordinator(configProvider, cmdLine, messagingClient, thingName);
            JsonObject fullConfig = layeredConfigCoordinator.loadEffective();
            
            if (fullConfig == null) {
                throw new ConfigurationException("No configuration found");
            }
            
            // Validate configuration
            validateConfiguration(fullConfig);
            
            // Create ConfigManager instance. The resolved platform (from the resolver; may be null in
            // test bring-up) is threaded in so the logging configurator can apply the platform-profile
            // default logging format — `json` (stdout-JSON sink) on KUBERNETES — when the component
            // config omits `logging.java_format` (FR-LOG-1/4, precedence FR-RT-3).
            List<CandidateValidationRunner.NamedValidator> namedValidators = namedValidators(validators);
            ConfigManager configManager = new ConfigManager(componentFullName, componentShortName,
                    thingName, configProvider, fullConfig, cmdLine.platform,
                    layeredConfigCoordinator, namedValidators, validationTimeout);

            // The provider may now deliver changes: the committed INITIAL snapshot exists and both
            // provider/coordinator point at the manager. A failed start is a startup failure and is
            // cleaned up before it can leave a partially-live watcher/subscription behind.
            configProvider.start();
            
            LOGGER.info("Configuration loaded from {}", configProvider.getConfigSource());
            return configManager;
            
        } catch (ConfigurationException e) {
            closeQuietly(configProvider);
            throw e;
        } catch (Exception e) {
            closeQuietly(configProvider);
            throw new ConfigurationException("Failed to create ConfigManager: " + e.getMessage(), e);
        }
    }

    private static List<CandidateValidationRunner.NamedValidator> namedValidators(
            Map<String, ConfigurationCandidateValidator> validators) {
        if (validators == null || validators.isEmpty()) {
            return List.of();
        }
        List<CandidateValidationRunner.NamedValidator> named = new ArrayList<>();
        validators.forEach((name, validator) -> {
            if (name == null || !VALIDATOR_NAME.matcher(name).matches()) {
                throw new IllegalArgumentException("configuration validator name must match "
                        + "^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$");
            }
            named.add(new CandidateValidationRunner.NamedValidator(
                    name, java.util.Objects.requireNonNull(validator,
                            "configuration validator must not be null")));
        });
        return List.copyOf(named);
    }

    private static void closeQuietly(ConfigProvider provider) {
        if (provider != null) {
            try {
                provider.close();
            } catch (RuntimeException closeError) {
                LOGGER.warn("Failed to clean up configuration provider after startup failure: {}",
                        closeError.toString());
            }
        }
    }
    
    /**
     * Resolves the thing name (identity) from the command line or environment, via the shared
     * {@link com.mbreissi.edgecommons.platform.PlatformResolver#resolveIdentity} (DESIGN-core §6.2
     * identity-injection site). Behavior is unchanged for the Phase-0 platforms.
     */
    private static String resolveThingName(ParsedCommandLine cmdLine) {
        return com.mbreissi.edgecommons.platform.PlatformResolver.resolveIdentity(
                cmdLine.thingName, cmdLine.platform, System.getenv());
    }
    
    /**
     * Validates configuration against schema.
     */
    private static void validateConfiguration(JsonObject config) throws ConfigurationException {
        try {
            ConfigurationValidator.validate(config);
        } catch (ConfigurationValidator.ConfigurationValidationException e) {
            throw new ConfigurationException("Configuration validation failed: " + e.getMessage(), e);
        }
    }
}
