/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.aws.proserve.ggcommons.config.ConfigManagerFactory;
import com.aws.proserve.ggcommons.di.ServiceFactory;
import com.aws.proserve.ggcommons.di.ServiceRegistry;
import com.aws.proserve.ggcommons.heartbeat.Heartbeat;
import com.aws.proserve.ggcommons.interfaces.IConfigurationService;
import com.aws.proserve.ggcommons.interfaces.IMessagingService;
import com.aws.proserve.ggcommons.interfaces.IMetricService;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.aws.proserve.ggcommons.metrics.MetricEmitter;
import org.apache.commons.cli.*;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;


public class GGCommons
{
    private static final Logger LOGGER = LogManager.getLogger(GGCommons.class);

    private ConfigManager configManager;
    private ServiceRegistry serviceRegistry;

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
            
            // Initialize service registry early so services can be injected
            initializeServiceRegistry();
            
            // Initialize other components - these will fail in production if services are unavailable
            MessagingClient.init(parsedCommandLine, receiveOwnMessages);
            
            // Inject messaging service into MetricEmitter before initialization
            MetricEmitter.setMessagingService(getService(IMessagingService.class));
            MetricEmitter.init(configManager);
            
            // Create heartbeat and inject services
            Heartbeat heartbeat = new Heartbeat(configManager);
            heartbeat.setMessagingService(getService(IMessagingService.class));
            heartbeat.setMetricService(getService(IMetricService.class));
            
            // Complete initialization - this must be the very last step
            // After this point, configuration changes will trigger listener notifications
            configManager.completeInitialization();
        } catch (Exception e) {
            LOGGER.error("Failed to initialize GGCommons: {}", e.getMessage(), e);
            System.exit(1);
        }
    }
    
    /**
     * Initialize for testing with pre-injected services.
     * This allows tests to inject mock services before any real initialization occurs.
     */
    protected void initForTesting(String componentName, String[] args) throws Exception {
        ParsedCommandLine parsedCommandLine = GGCommons.processArgs(componentName, args, null);
        configManager = ConfigManagerFactory.create(componentName, parsedCommandLine);
        
        if (serviceRegistry == null) {
            initializeServiceRegistry();
        }
        
        // Skip messaging, metrics, and heartbeat initialization for testing
        configManager.completeInitialization();
    }
    
    /**
     * Initializes the service registry and registers default service implementations.
     */
    protected void initializeServiceRegistry() {
        serviceRegistry = new ServiceRegistry();
        ServiceFactory.registerDefaultServices(serviceRegistry, configManager);
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
     * Returns the configuration service interface for this component.
     * 
     * @return The IConfigurationService interface
     */
    public IConfigurationService getConfigurationService()
    {
        return getService(IConfigurationService.class);
    }
    

    
    /**
     * Retrieves a service by its interface type.
     * 
     * @param serviceType The service interface class
     * @param <T> The service type
     * @return The service implementation, or null if not registered
     */
    public <T> T getService(Class<T> serviceType) {
        if (serviceRegistry == null) {
            throw new IllegalStateException("GGCommons not properly initialized");
        }
        return serviceRegistry.get(serviceType);
    }
    
    /**
     * Registers a custom service implementation.
     * This allows applications to override default service implementations.
     * 
     * @param serviceType The service interface class
     * @param implementation The service implementation
     * @param <T> The service type
     */
    public <T> void registerService(Class<T> serviceType, T implementation) {
        if (serviceRegistry == null) {
            throw new IllegalStateException("GGCommons not properly initialized");
        }
        serviceRegistry.register(serviceType, implementation);
    }
    
    /**
     * Returns the service registry for advanced service management.
     * 
     * @return The service registry instance
     */
    public ServiceRegistry getServiceRegistry() {
        return serviceRegistry;
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
                    System.exit(1);
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
