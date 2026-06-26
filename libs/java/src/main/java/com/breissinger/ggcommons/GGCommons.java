/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons;

import com.breissinger.ggcommons.config.ConfigManager;
import com.breissinger.ggcommons.config.ConfigManagerFactory;
import com.breissinger.ggcommons.heartbeat.Heartbeat;
import com.breissinger.ggcommons.heartbeat.HeartbeatBuilder;
import com.breissinger.ggcommons.messaging.MessagingClient;
import com.breissinger.ggcommons.messaging.MessagingClientBuilder;
import com.breissinger.ggcommons.metrics.MetricEmitter;
import com.breissinger.ggcommons.metrics.MetricEmitterBuilder;
import com.breissinger.ggcommons.streaming.StreamMetricsBridge;
import com.breissinger.ggcommons.streaming.StreamService;
import com.breissinger.ggcommons.credentials.CredentialMetricsBridge;
import com.breissinger.ggcommons.credentials.CredentialService;
import com.breissinger.ggcommons.credentials.Credentials;
import com.breissinger.ggcommons.credentials.SecretRefs;
import com.breissinger.ggcommons.parameters.DefaultParameterService;
import com.breissinger.ggcommons.parameters.ParameterService;
import com.breissinger.ggcommons.parameters.Parameters;
import com.breissinger.ggcommons.platform.Platform;
import com.breissinger.ggcommons.platform.PlatformResolver;
import com.breissinger.ggcommons.platform.ResolvedProfile;
import com.breissinger.ggcommons.platform.Transport;
import com.google.gson.JsonElement;
import com.google.gson.JsonParser;
import com.google.gson.JsonObject;
import org.apache.commons.cli.*;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.List;


public class GGCommons
{
    private static final Logger LOGGER = LogManager.getLogger(GGCommons.class);

    protected ConfigManager configManager;
    protected MessagingClient messagingClient;
    protected MetricEmitter metricEmitter;
    protected Heartbeat heartbeat;
    /** Telemetry streaming (the native ggstreamlog binding). Null when no {@code streaming} config. */
    protected StreamService streams;
    protected CredentialService credentials;
    /** Externalized config parameters (offline-first). Null when no {@code parameters} config. */
    protected ParameterService parameters;
    protected StreamMetricsBridge streamMetricsBridge;
    protected CredentialMetricsBridge credentialMetricsBridge;

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
    void init(String componentName, String[] args, Options appOptions, boolean receiveOwnMessages)
    {
        try {
            ParsedCommandLine parsedCommandLine = GGCommons.processArgs(componentName, args, appOptions);

            // Messaging must be initialized first: the GG_CONFIG / CONFIG_COMPONENT config sources
            // load the component configuration over IPC and therefore need the messaging client.
            messagingClient = MessagingClientBuilder.create(parsedCommandLine)
                    .withReceiveOwnMessages(receiveOwnMessages)
                    .build();

            // Initialize the config manager (passing messaging for IPC-backed config sources).
            configManager = ConfigManagerFactory.create(componentName, parsedCommandLine, messagingClient);

            metricEmitter = MetricEmitterBuilder.create(configManager)
                    .withMessagingService(messagingClient)
                    .build();

            heartbeat = HeartbeatBuilder.create(configManager)
                    .withMessagingService(messagingClient)
                    .withMetricService(metricEmitter)
                    .build();

            // Credentials / local vault first (mirrors Rust lib.rs): the vault must be open before
            // streaming consumes its config, so `$secret` refs in the streaming config resolve.
            initCredentials();
            // Parameters (externalized config): independent offline-first service paralleling
            // credentials. Opened after the vault so a remote source can reuse the same crypto.
            initParameters();
            // Telemetry streaming: only when a `streaming` config section is present (so components
            // that don't use it never load the native library).
            initStreaming();

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
     * Returns the telemetry streaming service for this component, or {@code null} if the component
     * configuration has no {@code streaming} section. Obtain a stream with
     * {@link StreamService#stream(String)} and append durable records to it.
     *
     * @return the native-backed {@link StreamService}, or {@code null} if streaming is not configured
     */
    public StreamService getStreams()
    {
        return streams;
    }

    /**
     * Opens telemetry streams from the {@code streaming} config section (if any), resolving config
     * templates, and starts the stats-to-metrics bridge. No-op when the section is absent.
     */
    private void initStreaming()
    {
        JsonObject full = configManager.getFullConfig();
        if (full == null || !full.has("streaming") || !full.get("streaming").isJsonObject())
        {
            return;
        }
        // Resolve {ThingName} etc. across the streaming section (buffer paths, Kinesis stream names).
        String streamingJson = configManager.resolveTemplate(full.getAsJsonObject("streaming").toString());
        // Resolve {"$secret": ...} refs from the vault (closes TELEMETRY_STREAMING.md §7) without
        // mutating the public config snapshot. Only when a credentials vault is open.
        if (credentials != null)
        {
            JsonElement resolved = SecretRefs.resolve(JsonParser.parseString(streamingJson), credentials);
            streamingJson = resolved.toString();
        }
        streams = StreamService.open(streamingJson);
        List<String> names = StreamService.streamNames(streamingJson);
        if (!names.isEmpty())
        {
            streamMetricsBridge = new StreamMetricsBridge(configManager, metricEmitter, streams, names);
        }
        LOGGER.info("Telemetry streaming initialized with {} stream(s)", names.size());
    }

    /**
     * Returns the credential service for this component, or {@code null} if the component
     * configuration has no {@code credentials} section. Mirrors Rust {@code gg.credentials()} /
     * Python {@code get_credentials()}.
     *
     * @return the {@link CredentialService}, or {@code null} if credentials are not configured
     */
    public CredentialService getCredentials()
    {
        return credentials;
    }

    /**
     * Opens the local vault from the {@code credentials} config section (if any), resolving path
     * templates. No-op when the section is absent.
     */
    private void initCredentials()
    {
        JsonObject full = configManager.getFullConfig();
        if (full == null || !full.has("credentials") || !full.get("credentials").isJsonObject())
        {
            return;
        }
        // Resolve {ThingName}/{ComponentFullName} in the vault path(s) before opening.
        String credentialsJson = configManager.resolveTemplate(full.getAsJsonObject("credentials").toString());
        // Transparently namespace every key by <thingName>/<componentName> (collision-free).
        String namespace = configManager.getThingName() + "/" + configManager.getComponentFullName();
        credentials = Credentials.open(JsonParser.parseString(credentialsJson).getAsJsonObject(), namespace);
        // Bridge non-sensitive credential stats into the configured metric target.
        credentialMetricsBridge = new CredentialMetricsBridge(configManager, metricEmitter, credentials);
        LOGGER.info("Credentials vault initialized");
    }

    /**
     * Returns the parameter service for this component, or {@code null} if the component
     * configuration has no {@code parameters} section. Mirrors Rust {@code gg.parameters()} /
     * Python {@code get_parameters()}.
     *
     * @return the {@link ParameterService}, or {@code null} if parameters are not configured
     */
    public ParameterService getParameters()
    {
        return parameters;
    }

    /**
     * Opens the parameter service from the {@code parameters} config section (if any), resolving path
     * templates in the persistent-cache path / key path. No-op when the section is absent. Parameter
     * keys are not namespaced (matching the Rust port); per-component isolation comes from the
     * templated {@code cache.path}.
     */
    private void initParameters()
    {
        JsonObject full = configManager.getFullConfig();
        if (full == null || !full.has("parameters") || !full.get("parameters").isJsonObject())
        {
            return;
        }
        // Resolve {ThingName}/{ComponentFullName} in the cache path / key path before opening.
        String parametersJson = configManager.resolveTemplate(full.getAsJsonObject("parameters").toString());
        parameters = Parameters.open(JsonParser.parseString(parametersJson).getAsJsonObject());
        LOGGER.info("Parameters service initialized");
    }

    /**
     * Shuts down this GGCommons instance, releasing background timers, threads and connections
     * held by the heartbeat, metric, messaging and configuration subsystems.
     */
    public void shutdown()
    {
        if (credentialMetricsBridge != null)
        {
            credentialMetricsBridge.close();
        }
        if (parameters instanceof DefaultParameterService dps)
        {
            dps.close();
        }
        if (streamMetricsBridge != null)
        {
            streamMetricsBridge.close();
        }
        if (streams != null)
        {
            streams.close();
        }
        if (heartbeat != null)
        {
            heartbeat.close();
        }
        if (metricEmitter != null)
        {
            metricEmitter.close();
        }
        if (messagingClient != null)
        {
            messagingClient.close();
        }
        if (configManager != null)
        {
            configManager.close();
        }
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
        // The legacy single-axis -m/--mode token is removed (DESIGN-core §6.1 / FR-RT-1). Reject it
        // explicitly with guidance to the new flags rather than letting it fall through as an
        // unrecognized option (which would be silently swallowed below).
        rejectLegacyModeFlag(args);

        ParsedCommandLine retVal = new ParsedCommandLine();
        CommandLineParser parser = new DefaultParser();
        Options options = appOptions == null ? new Options() : appOptions;
        Option helpOption = new Option("h", "help", false, "Display this help message");
        Option configOption = Option.builder("c")
                                    .longOpt("config")
                                    .hasArgs()
                                    .desc("Configuration source - one of: " +
                                            "'FILE <optional: file_path>', " +
                                            "'CONFIGMAP <optional: mount_dir> <optional: key>', " +
                                            "'ENV <optional: env_var_name>', " +
                                            "'SHADOW <optional: shadow_name>', " +
                                            "'GG_CONFIG <optional: component_name> <optional: config_key>', " +
                                            "'CONFIG_COMPONENT'\n" +
                                            "Default: from the resolved platform profile (GREENGRASS/HOST -> GG_CONFIG, KUBERNETES -> CONFIGMAP)")
                                    .build();
        Option platformOption = Option.builder()
                                       .longOpt("platform")
                                       .hasArg()
                                       .desc("Deployment platform - 'GREENGRASS', 'HOST', 'KUBERNETES' or 'auto' (default auto)")
                                       .build();
        Option transportOption = Option.builder()
                                       .longOpt("transport")
                                       .hasArgs()
                                       .desc("Messaging transport - 'IPC' or 'MQTT <messaging_config_path>' "
                                               + "(default: derived from the platform)")
                                       .build();
        Option thingOption = Option.builder("t")
                                    .longOpt("thing")
                                    .hasArg()
                                    .desc("Thing name to use (optional)")
                                    .build();
        options.addOption(helpOption);
        options.addOption(configOption);
        options.addOption(platformOption);
        options.addOption(transportOption);
        options.addOption(thingOption);

        PlatformResolver.ResolverInputs inputs;
        try {
            // parse the command line arguments
            CommandLine line = parser.parse(options, args);
            if (line.hasOption("h")) {
                HelpFormatter formatter = new HelpFormatter();
                formatter.printHelp(componentName, options);
                System.exit(0);
            }
            retVal.commandLine = line;

            // Explicit -c/--config args, or null (the resolver fills the platform-profile default).
            String[] configArgs = line.hasOption("c") ? line.getOptionValues("config") : null;

            Platform platformFlag = parsePlatform(line);
            Transport transportFlag = parseTransport(line, retVal);
            String thingFlag = line.hasOption("t") ? line.getOptionValue("thing") : null;

            inputs = new PlatformResolver.ResolverInputs(platformFlag, transportFlag, configArgs, thingFlag);
        }
        catch (ParseException exp) {
            LOGGER.error("Unexpected exception parsing command line options: {}", exp.getMessage());
            return retVal;
        }

        // Resolve the two runtime axes + the default config provider + identity from parse-time
        // inputs only (DESIGN-core §4 / §4.2). Validation failures (e.g. the IPC lock) propagate.
        ResolvedProfile resolved = PlatformResolver.resolveProfile(inputs, System.getenv());
        retVal.platform = resolved.platform();
        retVal.transport = resolved.transport();
        retVal.configArgs = resolved.configSource();
        retVal.thingName = resolved.identity();

        // Note: a missing MQTT messaging-config path is enforced when the MQTT provider is actually
        // built (MessagingClient), mirroring how the IPC provider only fails against a live Nucleus.
        // Parsing alone must not require it, so collaborators that inject a mock messaging client
        // (e.g. tests) can resolve args without supplying a broker config.

        return retVal;
    }

    /**
     * Rejects the removed {@code -m}/{@code --mode} flag with guidance to the new axes.
     */
    private static void rejectLegacyModeFlag(String[] args) {
        if (args == null) {
            return;
        }
        for (String arg : args) {
            // Catch attached forms too (--mode=X, -mX), which would otherwise slip past as an
            // unrecognized option and surface as a confusing half-parsed state / NPE.
            if (arg != null && ("--mode".equals(arg) || arg.startsWith("--mode=") || arg.startsWith("-m"))) {
                throw new IllegalArgumentException("The -m/--mode flag has been removed. Use "
                        + "--platform GREENGRASS|HOST|KUBERNETES and --transport IPC|MQTT instead "
                        + "(e.g. '-m STANDALONE <path>' becomes '--platform HOST --transport MQTT <path>').");
            }
        }
    }

    /**
     * Parses {@code --platform}; {@code auto} (or absent) yields {@code null} so the resolver
     * auto-detects.
     */
    private static Platform parsePlatform(CommandLine line) {
        if (!line.hasOption("platform")) {
            return null;
        }
        String raw = line.getOptionValue("platform").trim();
        if (raw.equalsIgnoreCase("auto")) {
            return null;
        }
        try {
            return Platform.valueOf(raw.toUpperCase());
        } catch (IllegalArgumentException e) {
            throw new IllegalArgumentException("Unknown platform '" + raw
                    + "'. Valid: GREENGRASS, HOST, KUBERNETES, auto.");
        }
    }

    /**
     * Parses {@code --transport [IPC|MQTT] <optional messaging-config path>}; absent yields
     * {@code null} so the resolver derives the transport from the platform. The optional second
     * value (the MQTT messaging-config path) is stashed on the {@link ParsedCommandLine}.
     */
    private static Transport parseTransport(CommandLine line, ParsedCommandLine retVal) {
        if (!line.hasOption("transport")) {
            return null;
        }
        String[] transportArgs = line.getOptionValues("transport");
        if (transportArgs.length > 1) {
            retVal.standaloneConfigPath = transportArgs[1];
        }
        try {
            return Transport.valueOf(transportArgs[0].toUpperCase());
        } catch (IllegalArgumentException e) {
            throw new IllegalArgumentException("Unknown transport '" + transportArgs[0]
                    + "'. Valid: IPC, MQTT.");
        }
    }
}
