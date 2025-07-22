/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.heartbeat.Heartbeat;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.aws.proserve.ggcommons.metrics.MetricEmitter;
import org.apache.commons.cli.*;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;


public class GGCommons
{
    private static final Logger LOGGER = LogManager.getLogger(GGCommons.class);

    private ConfigManager configManager;

    /**
     * Constructs a new GGCommons instance with the given component name and command line arguments.
     * 
     * @param componentName The name of the Greengrass component
     * @param args Command line arguments passed to the component
     */
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
     */
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
     */
    public GGCommons(String componentName, String[] args, Options appOptions, boolean receiveOwnMessages)
    {
        init(componentName, args, appOptions, receiveOwnMessages);
    }

    /**
     * Initializes the GGCommons instance with the specified parameters.
     * This method sets up the core components including messaging, configuration, metrics, and heartbeat.
     *
     * @param componentName The name of the Greengrass component
     * @param args Command line arguments to process
     * @param appOptions Custom application options
     * @param receiveOwnMessages Flag indicating whether to receive own messages
     */
    private void init(String componentName, String[] args, Options appOptions, boolean receiveOwnMessages)
    {
        ParsedCommandLine parsedCommandLine = GGCommons.processArgs(componentName, args, appOptions);
        MessagingClient.init(parsedCommandLine, receiveOwnMessages);
        configManager = new ConfigManager(componentName, parsedCommandLine);
        MetricEmitter.init(configManager);
        new Heartbeat(configManager);
    }

    /**
     * Returns the configuration manager instance for this component.
     * 
     * @return The ConfigManager instance managing this component's configuration
     */
    public ConfigManager getConfigManager()
    {
        return configManager;
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
        Option messagingOption = Option.builder("m")
                                       .longOpt("messaging")
                                       .hasArgs()
                                       .desc("Messaging system - one of: IPC, MQTT <host> <port> <creds dir>\n" +
                                               "Default: IPC")
                                       .build();
        Option thingOption = Option.builder("t")
                                    .longOpt("thing")
                                    .hasArg()
                                    .desc("Thing name to use (optional)")
                                    .build();
        options.addOption(helpOption);
        options.addOption(configOption);
        options.addOption(messagingOption);
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

            String[] messagingArgs;
            if (line.hasOption("m")) {
                messagingArgs = line.getOptionValues("messaging");
            } else {
                LOGGER.info("No com.aws.proseve.ggcommons.messaging system specified. Assuming IPC");
                messagingArgs = new String[] {"IPC"};
            }
            retVal.messagingArgs = messagingArgs;

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
