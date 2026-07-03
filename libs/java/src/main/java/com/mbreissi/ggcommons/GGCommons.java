/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons;

import com.mbreissi.ggcommons.config.ConfigManager;
import com.mbreissi.ggcommons.config.ConfigManagerFactory;
import com.mbreissi.ggcommons.config.EffectiveConfigPublisher;
import com.mbreissi.ggcommons.config.HealthConfiguration;
import com.mbreissi.ggcommons.health.HealthServer;
import com.mbreissi.ggcommons.heartbeat.Heartbeat;
import com.mbreissi.ggcommons.heartbeat.HeartbeatBuilder;
import com.mbreissi.ggcommons.messaging.MessagingClient;
import com.mbreissi.ggcommons.messaging.MessagingClientBuilder;
import com.mbreissi.ggcommons.metrics.MetricEmitter;
import com.mbreissi.ggcommons.metrics.MetricEmitterBuilder;
import com.mbreissi.ggcommons.streaming.StreamMetricsBridge;
import com.mbreissi.ggcommons.streaming.StreamService;
import com.mbreissi.ggcommons.credentials.CredentialMetricsBridge;
import com.mbreissi.ggcommons.credentials.CredentialService;
import com.mbreissi.ggcommons.credentials.Credentials;
import com.mbreissi.ggcommons.credentials.SecretRefs;
import com.mbreissi.ggcommons.parameters.DefaultParameterService;
import com.mbreissi.ggcommons.parameters.ParameterService;
import com.mbreissi.ggcommons.parameters.Parameters;
import com.mbreissi.ggcommons.platform.Platform;
import com.mbreissi.ggcommons.platform.PlatformResolver;
import com.mbreissi.ggcommons.platform.ResolvedProfile;
import com.mbreissi.ggcommons.platform.Transport;
import com.mbreissi.ggcommons.uns.RepublishListener;
import com.mbreissi.ggcommons.uns.Uns;
import com.google.gson.JsonElement;
import com.google.gson.JsonParser;
import com.google.gson.JsonObject;
import org.apache.commons.cli.*;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Collection;
import java.util.List;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicBoolean;


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
     * The library-owned {@code cfg} publisher (UNS-CANONICAL-DESIGN §4.3): announces the effective
     * (redacted) configuration on {@code ecv1/{device}/{component}/main/cfg} at startup and on
     * every configuration change.
     */
    protected EffectiveConfigPublisher effectiveConfigPublisher;
    /**
     * The library-owned {@code _bcast} republish listener (DESIGN-uns §9.3/§9.4, the late-join
     * lever): subscribes {@code ecv1/{device}/_bcast/main/cmd/republish-state|republish-cfg} on
     * the primary connection and re-announces {@code state}/{@code cfg} out of band (jittered,
     * coalesced) when the {@code uns-bridge} — or a console — broadcasts a republish command.
     */
    protected RepublishListener republishListener;

    /**
     * The component-identity-bound UNS topic builder (instance
     * {@code MessageIdentity.DEFAULT_INSTANCE}), lazily bound on first {@link #getUns()} from the
     * resolved component identity + {@code topic.includeRoot} (UNS-CANONICAL-DESIGN §2).
     */
    private volatile Uns uns;
    /**
     * Cached per-id instance handles (UNS-CANONICAL-DESIGN §3, D-U3): {@link #instance(String)}
     * returns the same {@link GgInstance} for the same id.
     */
    private final ConcurrentHashMap<String, GgInstance> instanceHandles = new ConcurrentHashMap<>();

    /**
     * The HTTP health/readiness server (FR-HB-1). Non-null only when the health server is enabled
     * (explicit {@code health.enabled} ▸ default-on for KUBERNETES). Closed by {@link #shutdown()}.
     */
    protected HealthServer healthServer;
    /**
     * App-settable readiness gate (FR-HB-2), defaulting to {@code true}: a component is ready as soon
     * as messaging connects, but an app can gate readiness on its own required subscriptions by
     * calling {@link #setReady(boolean) setReady(false)} early and {@code setReady(true)} once ready.
     * Part of {@code readyz = connected && readyFlag && !shuttingDown}.
     */
    private volatile boolean readyFlag = true;
    /**
     * Flipped to {@code true} at the very start of the shutdown / SIGTERM path so {@code /readyz}
     * returns 503 immediately (drains traffic) before teardown begins (FR-HB-2).
     */
    private volatile boolean shuttingDown = false;
    /** Guards the close chain so {@link #shutdown()} is idempotent across the app + the SIGTERM hook. */
    private final AtomicBoolean shutdownComplete = new AtomicBoolean(false);
    /** The library-owned SIGTERM/SIGINT shutdown hook (FR-HB-2); removed by an app-driven shutdown. */
    private Thread shutdownHook;

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

            // UNS-CANONICAL-DESIGN §5 / D-U5 (§1.5 init order): late-bind the request() default
            // deadline from messaging.requestTimeoutSeconds now that the ConfigManager exists.
            // The messaging client is built BEFORE config loads (the IPC-backed config sources
            // need it), so until this bind the provider's built-in 30 s applied — deliberately,
            // giving the CONFIG_COMPONENT bootstrap request a deadline instead of hanging forever.
            messagingClient.setDefaultRequestTimeout(configManager.getMessagingRequestTimeout());

            // §4.1 / D-U24: late-bind the reserved-class guard's topic.includeRoot flag the same
            // way (default false pre-bind - nothing publishes rooted topics pre-config). D-U27: bind
            // the EFFECTIVE root (includeRoot AND a multi-level hierarchy) so the guard's position-5
            // check agrees with topic-building, which no-ops includeRoot on a single-level hierarchy
            // (D-U25); otherwise a warned single-level+includeRoot misconfig would false-positive on a
            // legit app/evt/data channel whose first token is a reserved word.
            messagingClient.setGuardIncludeRoot(
                    configManager.isTopicIncludeRoot()
                            && configManager.getComponentIdentity().getHier().size() >= 2);

            // Logging is now (re)configured (the ConfigManager constructor applied the config and
            // reconfigured Log4j2). Re-emit the startup facts that were resolved/connected BEFORE
            // logging was ready, so they actually reach the log: the resolver summary and, if the
            // messaging client is connected, the messaging connectivity line.
            LOGGER.info("platform resolved: platform={} transport={} configSource={} identity={}",
                    parsedCommandLine.platform, parsedCommandLine.transport,
                    parsedCommandLine.configArgs[0], parsedCommandLine.thingName);
            if (messagingClient.connected()) {
                LOGGER.info("messaging connected (transport={})", parsedCommandLine.transport);
            }

            metricEmitter = MetricEmitterBuilder.create(configManager)
                    .withMessagingService(messagingClient)
                    .withPlatform(parsedCommandLine.platform)
                    .build();

            heartbeat = HeartbeatBuilder.create(configManager)
                    .withMessagingService(messagingClient)
                    .withMetricService(metricEmitter)
                    .build();

            // Credentials / local vault first (mirrors Rust lib.rs): the vault must be open before
            // streaming consumes its config, so `$secret` refs in the streaming config resolve.
            initCredentials(parsedCommandLine.platform);
            // Parameters (externalized config): independent offline-first service paralleling
            // credentials. Opened after the vault so a remote source can reuse the same crypto.
            initParameters();
            // Telemetry streaming: only when a `streaming` config section is present (so components
            // that don't use it never load the native library).
            initStreaming();

            // Complete initialization - this must be the very last step
            // After this point, configuration changes will trigger listener notifications
            configManager.completeInitialization();

            // §4.3: announce the effective (redacted) configuration on the UNS cfg topic - the
            // startup push; the publisher re-announces on every configuration change. Best-effort
            // (publishNow never throws).
            effectiveConfigPublisher = new EffectiveConfigPublisher(configManager, messagingClient);
            effectiveConfigPublisher.publishNow();

            // §9.3/§9.4: subscribe the own-device _bcast republish topics on the primary
            // connection so the uns-bridge's reconnect-rehydration broadcast (and a console's
            // explicit republish) gets a jittered, coalesced state/cfg re-announce. Always on
            // (no config surface); best-effort start (a failure disables the listener only).
            republishListener = new RepublishListener(configManager, messagingClient,
                    heartbeat::publishStateNow, effectiveConfigPublisher::publishNow);
            republishListener.start();

            // FR-HB-1: start the HTTP health endpoint (default-on for KUBERNETES; opt-in elsewhere),
            // and FR-HB-2: wire SIGTERM/SIGINT to the graceful, idempotent shutdown so a kubelet (or
            // the Greengrass Nucleus) terminating the process flips /readyz -> 503, unsubscribes
            // every tracked subscription and bounded-closes the runtime before exit.
            startHealthServer(parsedCommandLine.platform);
            installShutdownHook();
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
     * Returns the UNS topic builder + validator bound to this component's resolved identity
     * (instance {@code "main"}) and its {@code topic.includeRoot} setting
     * (UNS-CANONICAL-DESIGN §2). For instance-scoped topics use
     * {@link #instance(String)}{@code .uns()}.
     *
     * @return the component-bound {@link Uns}
     * @throws IllegalStateException when called before initialization completes (no resolved
     *                               component identity yet)
     */
    public Uns getUns()
    {
        Uns bound = uns;
        if (bound == null)
        {
            ConfigManager cm = requireResolvedIdentity();
            bound = new Uns(cm.getComponentIdentity(), cm.isTopicIncludeRoot());
            uns = bound;
        }
        return bound;
    }

    /**
     * Returns the instance-scoped handle for an instance token (UNS-CANONICAL-DESIGN §3, D-U3):
     * a {@link GgInstance} whose {@code uns()} mints topics with — and whose
     * {@code newMessage(...)} stamps envelopes with — this instance token. The token is validated
     * against the §2.2 token rule; handles are cached per id, so repeated calls return the same
     * object. The id is deliberately NOT verified against the configured
     * {@code component.instances[]} (instances may be created dynamically) — an unknown id is
     * only logged at DEBUG as a diagnostic aid.
     *
     * @param instanceId the instance token (e.g. {@code "kep1"})
     * @return the cached handle for this instance token
     * @throws com.mbreissi.ggcommons.uns.UnsValidationException when the token violates the
     *                                                           §2.2 token rule
     * @throws IllegalStateException when called before initialization completes
     */
    public GgInstance instance(String instanceId)
    {
        Uns.checkToken(instanceId, "instance id");
        ConfigManager cm = requireResolvedIdentity();
        return instanceHandles.computeIfAbsent(instanceId, id -> {
            Collection<String> configured = cm.getInstanceIds();
            if (configured == null || !configured.contains(id))
            {
                LOGGER.debug("instance('{}'): id is not among the configured component.instances[]"
                        + " ids {} - creating a dynamic instance handle", id, configured);
            }
            return new GgInstance(id, cm, cm.isTopicIncludeRoot());
        });
    }

    /**
     * Guards the UNS accessors: they need the config manager and its resolved component identity,
     * which exist only after {@link #init} has constructed the {@link ConfigManager}.
     */
    private ConfigManager requireResolvedIdentity()
    {
        if (configManager == null || configManager.getComponentIdentity() == null)
        {
            throw new IllegalStateException("GGCommons is not initialized: the component"
                    + " configuration (and its resolved UNS identity) is not available yet");
        }
        return configManager;
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
     * templates. No-op when the section is absent — the {@code platform}-derived default key provider
     * is applied <em>only</em> when a {@code credentials} section is present, so this never
     * auto-enables credentials (FR-CRED-6).
     *
     * @param platform the resolved deployment platform, selecting the platform-profile default
     *                 key-provider type ({@code env} on KUBERNETES) when the config omits an explicit
     *                 {@code credentials.vault.keyProvider.type}
     */
    private void initCredentials(Platform platform)
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
        // FR-RT-3 middle tier: when keyProvider.type is absent, default to the platform-profile
        // provider (env on KUBERNETES); the library default 'file' applies when this is null.
        String defaultKeyProvider = PlatformResolver.profileCredentialsKeyProvider(platform);
        credentials = Credentials.open(JsonParser.parseString(credentialsJson).getAsJsonObject(), namespace,
                defaultKeyProvider);
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
     * Sets the application-controlled readiness flag (FR-HB-2). The flag defaults to {@code true}, so
     * a component is reported ready by {@code /readyz} as soon as messaging connects. An app that must
     * not receive traffic until its own required subscriptions are established should call
     * {@code setReady(false)} early (e.g. before subscribing) and {@code setReady(true)} once ready.
     * It contributes to the readiness predicate {@code connected && readyFlag && !shuttingDown}; it
     * cannot force readiness while messaging is disconnected or during shutdown.
     *
     * @param ready the new application readiness state
     */
    public void setReady(boolean ready)
    {
        this.readyFlag = ready;
        LOGGER.debug("Application readiness flag set to {}", ready);
    }

    /**
     * Liveness signal for {@code GET /livez} (FR-HB-1): always {@code true} while this object's
     * methods can execute (the process is alive). It deliberately <b>never</b> consults the broker or
     * any external dependency — a broker outage must not fail liveness and trigger kubelet restart
     * storms.
     *
     * @return {@code true} (the process is alive)
     */
    boolean isLive()
    {
        return true;
    }

    /**
     * Whether the messaging transport is connected — the messaging input to the readiness model.
     * {@code false} when no messaging client is wired (treated as not-ready).
     *
     * @return {@code true} if messaging is connected
     */
    boolean messagingConnected()
    {
        return messagingClient != null && messagingClient.connected();
    }

    /**
     * Readiness signal for {@code GET /readyz} and {@code GET /startupz} (FR-HB-2):
     * {@code messagingConnected() && readyFlag && !shuttingDown}. Returns {@code false} (probe 503)
     * during startup before messaging connects, when the app has gated readiness via
     * {@link #setReady(boolean)}, and immediately on shutdown/SIGTERM.
     *
     * @return {@code true} when the component is ready to serve traffic
     */
    boolean isReadyz()
    {
        return messagingConnected() && readyFlag && !shuttingDown;
    }

    /**
     * Resolves whether the health server should start (FR-HB-1, precedence FR-RT-3): an explicit
     * {@code health.enabled} from the config wins ▸ else the platform-profile default ({@code true} on
     * KUBERNETES via {@link PlatformResolver#profileHealthEnabled}) ▸ else {@code false}.
     *
     * @param health   the parsed health configuration
     * @param platform the resolved deployment platform (may be {@code null})
     * @return {@code true} if the health server should be started
     */
    static boolean resolveHealthEnabled(HealthConfiguration health, Platform platform)
    {
        if (health != null && health.isEnabledExplicitlySet())
        {
            return health.isEnabled();  // explicit config wins (top tier)
        }
        return PlatformResolver.profileHealthEnabled(platform);  // platform-profile default
    }

    /**
     * Starts the HTTP health server (FR-HB-1) when enabled by {@link #resolveHealthEnabled}. A
     * bind/start failure is logged and swallowed — a health-endpoint problem must never crash the
     * component. No-op when disabled (the GREENGRASS / HOST default without {@code health.enabled}).
     *
     * @param platform the resolved deployment platform (selects the default-on KUBERNETES behavior)
     */
    void startHealthServer(Platform platform)
    {
        HealthConfiguration health = configManager.getHealthConfig();
        if (!resolveHealthEnabled(health, platform))
        {
            LOGGER.debug("Health server disabled (platform={}, explicit={})",
                    platform, health != null && health.isEnabledExplicitlySet());
            return;
        }
        try
        {
            healthServer = new HealthServer(health.getPort(), health.getLivenessPath(),
                    health.getReadinessPath(), health.getStartupPath(), this::isLive, this::isReadyz);
            LOGGER.info("Health server listening on 0.0.0.0:{} ({}=livez, {}=readyz, {}=startupz)",
                    healthServer.getPort(), health.getLivenessPath(), health.getReadinessPath(),
                    health.getStartupPath());
        }
        catch (Exception e)
        {
            LOGGER.error("Failed to start health server on port {} (continuing without it): {}",
                    health.getPort(), e.getMessage(), e);
        }
    }

    /**
     * Installs the library-owned SIGTERM/SIGINT shutdown hook (FR-HB-2). The JVM runs shutdown hooks
     * on SIGTERM (the kubelet's termination signal) and SIGINT, so this is where the kubelet's
     * graceful-stop is wired to {@link #onShutdownSignal()}: flip {@code /readyz} to 503, then run the
     * idempotent close chain. The JVM exits 0 after the hook completes (no explicit {@code exit} — and
     * calling {@code System.exit} inside a hook would deadlock). An app-initiated {@link #shutdown()}
     * deregisters this hook to avoid a redundant second run.
     */
    private void installShutdownHook()
    {
        shutdownHook = new Thread(this::onShutdownSignal, "ggcommons-shutdown");
        try
        {
            Runtime.getRuntime().addShutdownHook(shutdownHook);
        }
        catch (IllegalStateException e)
        {
            // The JVM is already shutting down; nothing to wire.
            LOGGER.debug("Could not register shutdown hook (JVM already shutting down): {}", e.getMessage());
        }
    }

    /**
     * The SIGTERM/SIGINT handler body (FR-HB-2). Flips {@code shuttingDown} so {@code /readyz} returns
     * 503 immediately, then runs the idempotent shutdown chain. Does not remove the hook (it is
     * running inside it) and does not call {@code System.exit} (the JVM exits 0 once hooks finish).
     */
    void onShutdownSignal()
    {
        LOGGER.info("Received termination signal; shutting down GGCommons gracefully");
        shuttingDown = true;
        shutdownInternal(false);
    }

    /**
     * Shuts down this GGCommons instance, releasing background timers, threads and connections
     * held by the health, heartbeat, metric, messaging and configuration subsystems. Idempotent and
     * safe to call multiple times (e.g. by the app and the SIGTERM hook). Flips readiness to 503
     * before tearing down (FR-HB-2).
     */
    public void shutdown()
    {
        shutdownInternal(true);
    }

    /**
     * The shared, idempotent shutdown chain. The first caller wins the {@link #shutdownComplete}
     * CAS and runs the teardown; later callers (the other of app/SIGTERM-hook) return immediately.
     *
     * @param removeHook whether to deregister the SIGTERM hook (true for an app-driven shutdown;
     *                   false when invoked from within the hook itself)
     */
    private void shutdownInternal(boolean removeHook)
    {
        // Flip readiness to 503 first so an in-flight /readyz probe drains traffic even if the close
        // chain below takes a moment.
        shuttingDown = true;
        if (!shutdownComplete.compareAndSet(false, true))
        {
            return;  // already shut down — idempotent
        }
        if (removeHook && shutdownHook != null)
        {
            try
            {
                Runtime.getRuntime().removeShutdownHook(shutdownHook);
            }
            catch (IllegalStateException ignored)
            {
                // JVM shutdown already in progress; the hook will be (or is being) run anyway.
            }
        }
        // Stop accepting health probes first (the readiness flag already reports 503).
        if (healthServer != null)
        {
            healthServer.close();
        }
        // Unsubscribe the _bcast republish topics while messaging is still up (the
        // unsubscribe-before-exit rule) and stop reacting to republish broadcasts mid-teardown.
        if (republishListener != null)
        {
            republishListener.close();
        }
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
                                            "Default: from the resolved platform profile (GREENGRASS -> GG_CONFIG, HOST -> FILE, KUBERNETES -> CONFIGMAP)")
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
            // parseTransport stashes the explicit --transport MQTT <path> payload on retVal; pass it
            // to the resolver so it can apply the FR-MSG-1 CONFIGMAP default when it is absent.
            Transport transportFlag = parseTransport(line, retVal);
            String thingFlag = line.hasOption("t") ? line.getOptionValue("thing") : null;

            inputs = new PlatformResolver.ResolverInputs(
                    platformFlag, transportFlag, configArgs, thingFlag, retVal.standaloneConfigPath);
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
        // FR-MSG-1: the resolved messaging-config path is the explicit --transport MQTT <path> when
        // given, else (under CONFIGMAP+MQTT) the default ConfigMap file path; else null. Overwrites the
        // explicit value parseTransport stashed (a no-op when it was the source of the resolved value).
        retVal.standaloneConfigPath = resolved.messagingConfigPath();

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
