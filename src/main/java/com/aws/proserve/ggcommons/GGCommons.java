/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.config.ConfigManagerFactory;
import com.aws.proserve.ggcommons.heartbeat.Heartbeat;
import com.aws.proserve.ggcommons.heartbeat.HeartbeatBuilder;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.aws.proserve.ggcommons.messaging.MessagingClientBuilder;
import com.aws.proserve.ggcommons.metrics.MetricEmitter;
import com.aws.proserve.ggcommons.metrics.MetricEmitterBuilder;
import org.apache.commons.cli.*;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;


public class GGCommons
{
    private static final Logger LOGGER = LogManager.getLogger(GGCommons.class);

    protected ConfigManager configManager;
    protected MessagingClient messagingClient;
    protected MetricEmitter metricEmitter;
    protected Heartbeat heartbeat;

    /**
     * Constructs a new GGCommons instance with the given component name and command line arguments.
     * 
     * @param componentName The name of the Greengrass component
     * @param args Command line arguments passed to the component
     * @deprecated Use {@link GGCommonsBuilder#create(String)} instead
     */
    @Deprecated
    public GGCommons(String componentName, String[] args)
    {
        init(componentName, args, null, true);
    }

    /**
     * Constructs a new GGCommons instance with custom application options.
     * 
     * @param componentName The name of the Greengrass component
     * @param args Command line arguments passed to the component
     * @param appOptions Custom options for the application
     * @deprecated Use {@link GGCommonsBuilder#create(String)} instead
     */
    @Deprecated
    public GGCommons(String componentName, String[] args, Options appOptions)
    {
        init(componentName, args, appOptions, true);
    }

    /**
     * Constructs a new GGCommons instance with custom options and message reception settings.
     * 
     * @param componentName The name of the Greengrass component
     * @param args Command line arguments passed to the component
     * @param appOptions Custom options for the application
     * @param receiveOwnMessages Flag to determine if the component should receive its own messages.  Applies only when
     *                           messaging target is IPC
     * @deprecated Use {@link GGCommonsBuilder#create(String)} instead
     */
    @Deprecated
    public GGCommons(String componentName, String[] args, Options appOptions, boolean receiveOwnMessages)
    {
        init(componentName, args, appOptions, receiveOwnMessages);
    }

    /**
     * Protected constructor for testing that allows service injection before initialization.
     */
    protected GGCommons() {
        // Empty constructor for testing
    }
    
    /**
     * Initializes the GGCommons instance with the specified parameters.
     * This method sets up the core components including messaging, configuration, metrics, and heartbeat.
     *
     * @param componentName The name of the Greengrass component
     * @param args Command line arguments to process
     * @param appOptions Custom application options
     * @param receiveOwnMessages Flag indicating whether to receive own messages (used only for Greengrass components)
     */
    private void init(String componentName, String[] args, Options appOptions, boolean receiveOwnMessages)
    {
        try {
            ParsedCommandLine parsedCommandLine = GGCommons.processArgs(componentName, args, appOptions);

            // Initialize config manager first
            configManager = ConfigManagerFactory.create(componentName, parsedCommandLine);

            // Wire the messaging, metric and heartbeat subsystems directly (no service locator).
            messagingClient = MessagingClientBuilder.create(parsedCommandLine)
                    .withReceiveOwnMessages(receiveOwnMessages)
                    .build();

            metricEmitter = MetricEmitterBuilder.create(configManager)
                    .withMessagingService(messagingClient)
                    .build();

            heartbeat = HeartbeatBuilder.create(configManager)
                    .withMessagingService(messagingClient)
                    .withMetricService(metricEmitter)
                    .build();

            // Complete initialization - this must be the very last step
            // After this point, configuration changes will trigger listener notifications
            configManager.completeInitialization();
        } catch (Exception e) {
            LOGGER.error("Failed to initialize GGCommons: {}", e.getMessage(), e);
            throw new RuntimeException("Failed to initialize GGCommons: " + e.getMessage(), e);
        }
    }
    
    /**
     * Returns the configuration manager for this component.
     *
     * @return The ConfigManager managing this component's configuration
     */
    public ConfigManager getConfigManager()
    {
        return configManager;
    }

    /**
     * Returns the messaging client for this component.
     *
     * @return The MessagingClient for local IPC / IoT Core publish, subscribe and request/reply
     */
    public MessagingClient getMessaging()
    {
        return messagingClient;
    }

    /**
     * Returns the metric emitter for this component.
     *
     * @return The MetricEmitter for defining and emitting metrics
     */
    public MetricEmitter getMetrics()
    {
        return metricEmitter;
    }

    /**
     * Processes command line arguments for a Greengrass component.
     * 
     * @param componentName The name of the Greengrass component
     * @param args Command line arguments to process
     * @param appOptions Custom application options to consider during processing
     * @return A ParsedCommandLine object containing the processed arguments
     */
    public static ParsedCommandLine processArgs(String componentName, String[] args, Options appOptions) {
        ParsedCommandLine retVal = new ParsedCommandLine();
        CommandLineParser parser = new DefaultParser();
        Options options = appOptions == null ? new Options() : appOptions;
        Option helpOption = new Option("h", "help", false, "Display this help message");
        Option configOption = Option.builder("c")
                                    .longOpt("config")
                                    .hasArgs()
                                    .desc("Configuration source - one of: " +
                                            "'FILE <optional: file_path>', " +
                                            "'ENV <optional: env_var_name>', " +
                                            "'SHADOW <optional: shadow_name>', " +
                                            "'GG_CONFIG <optional: component_name> <optional: config_key>'" +
                                            "'CONFIG_COMPONENT'\n"+
                                            "Default: GG_CONFIG")
                                    .build();
        Option modeOption = Option.builder("m")
                                       .longOpt("mode")
                                       .hasArgs()
                                       .desc("Runtime mode - 'GREENGRASS' (default) or 'STANDALONE <config_file_path>'")
                                       .build();
        Option thingOption = Option.builder("t")
                                    .longOpt("thing")
                                    .hasArg()
                                    .desc("Thing name to use (optional)")
                                    .build();
        options.addOption(helpOption);
        options.addOption(configOption);
        options.addOption(modeOption);
        options.addOption(thingOption);

        try {
            // parse the command line arguments
            CommandLine line = parser.parse(options, args);
            if (line.hasOption("h")) {
                HelpFormatter formatter = new HelpFormatter();
                formatter.printHelp(componentName, options);
                System.exit(0);
            }
            retVal.commandLine = line;

            String[] configArgs;
            if (line.hasOption("c")) {
                configArgs = line.getOptionValues("config");
            } else {
                LOGGER.info("No configuration source specified. Assuming GG_CONFIG");
                configArgs = new String[]{"GG_CONFIG"};
            }
            retVal.configArgs = configArgs;

            String[] modeArgs;
            if (line.hasOption("m")) {
                modeArgs = line.getOptionValues("mode");
            } else {
                LOGGER.info("No mode specified. Assuming GREENGRASS");
                modeArgs = new String[] {"GREENGRASS"};
            }
            
            if (modeArgs[0].equalsIgnoreCase("STANDALONE")) {
                retVal.mode = ParsedCommandLine.Mode.STANDALONE;
                if (modeArgs.length > 1) {
                    retVal.standaloneConfigPath = modeArgs[1];
                } else {
                    LOGGER.error("STANDALONE mode requires config file path");
                    throw new IllegalArgumentException("STANDALONE mode requires a config file path");
                }
            } else {
                retVal.mode = ParsedCommandLine.Mode.GREENGRASS;
            }

            if (line.hasOption("t")) {
                retVal.thingName = line.getOptionValue("thing");
            }
        }
        catch (ParseException exp) {
            LOGGER.error("Unexpected exception parsing command line options: {}", exp.getMessage());
        }

        return retVal;
    }
}
