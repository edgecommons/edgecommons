package com.aws.proserve.ggcommons;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.heartbeat.Heartbeat;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import org.apache.commons.cli.*;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

public class GGCommons
{
    private static final Logger LOGGER = LogManager.getLogger(GGCommons.class);

    private ConfigManager configManager;

    public GGCommons(String componentName, String[] args)
    {
        init(componentName, args, null, true);
    }

    public GGCommons(String componentName, String[] args, Options appOptions)
    {
        init(componentName, args, appOptions, true);
    }

    public GGCommons(String componentName, String[] args, Options appOptions, boolean receiveOwnMessages)
    {
        init(componentName, args, appOptions, receiveOwnMessages);
    }

    private void init(String componentName, String[] args, Options appOptions, boolean receiveOwnMessages)
    {
        ParsedCommandLine parsedCommandLine = GGCommons.processArgs(componentName, args, appOptions);
        MessagingClient.init(parsedCommandLine.messagingArgs, receiveOwnMessages);
        configManager = new ConfigManager(componentName, parsedCommandLine.configArgs);
        new Heartbeat(configManager);
    }

    public ConfigManager getConfigManager()
    {
        return configManager;
    }

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
                                       .desc("Messaging system - one of: IPC, MQTT <host> <port>\n" +
                                               "Default: IPC")
                                       .build();
        options.addOption(helpOption);
        options.addOption(configOption);
        options.addOption(messagingOption);

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
        }
        catch (ParseException exp) {
            LOGGER.error("Unexpected exception parsing command line options: {}", exp.getMessage());
        }

        return retVal;
    }
}
