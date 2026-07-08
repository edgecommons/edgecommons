/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

import com.mbreissi.edgecommons.ParsedCommandLine;
import com.mbreissi.edgecommons.config.provider.ConfigProvider;
import com.mbreissi.edgecommons.config.provider.ConfigProviderBuilder;
import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.mbreissi.edgecommons.platform.Platform;
import com.mbreissi.edgecommons.platform.PlatformResolver;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.apache.logging.log4j.Level;
import org.apache.logging.log4j.core.LoggerContext;
import org.apache.logging.log4j.core.appender.ConsoleAppender;
import org.apache.logging.log4j.core.config.Configuration;
import org.apache.logging.log4j.core.config.Configurator;
import org.apache.logging.log4j.core.config.builder.api.*;
import org.apache.logging.log4j.core.config.builder.impl.BuiltConfiguration;

import java.util.ArrayList;
import java.util.Collection;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.regex.Pattern;

import static org.apache.logging.log4j.core.config.builder.api.ConfigurationBuilderFactory.newConfigurationBuilder;


/**
 * Manages configuration for Greengrass components including global settings, instance-specific configurations,
 * logging, metrics, heartbeat, and tag configurations. This class provides methods to access and modify
 * component configurations and handles configuration change notifications.
 */
public class ConfigManager
{
    private static final Logger LOGGER = LogManager.getLogger(ConfigManager.class);

    ConfigProvider configProvider;
    private LayeredConfigCoordinator layeredConfigCoordinator;
    protected final String componentName;
    protected final String componentFullName;
    protected final String thingName;
    /**
     * The resolved deployment platform (or {@code null} when unknown, e.g. test/subclass bring-up).
     * Used only to source the platform-profile default logging format (FR-LOG-1/4, precedence
     * FR-RT-3): the resolved platform is known before the component config loads, so the logging
     * configurator can default to the stdout-JSON sink on KUBERNETES when the config omits
     * {@code logging.java_format}. See {@link #reconfigureLogging()}.
     */
    protected final Platform platform;
    /**
     * The component's resolved UNS identity (hierarchy + identity values + device + component
     * token, instance {@value MessageIdentity#DEFAULT_INSTANCE}), resolved <b>once at
     * construction</b> from the component's OWN config (no shared config) — see
     * {@link #getComponentIdentity()}. {@code null} only on the protected test/subclass
     * bring-up constructor.
     */
    private final MessageIdentity componentIdentity;
    protected final CopyOnWriteArrayList<ConfigurationChangeListener> configChangeListeners = new CopyOnWriteArrayList<>();
    private boolean initializing = true;
    /**
     * The full effective configuration as last APPLIED: seeded by the constructor and refreshed
     * by every accepted {@link #applyConfig} (hot reload / {@code set-config} push /
     * {@link #reloadFromProvider}), so {@link #getFullConfig()} always reflects the live config
     * — the effective-config publisher and the {@code get-configuration} verb read it.
     */
    protected JsonObject fullConfig;
    protected TagConfiguration tagConfig;
    protected HeartbeatConfiguration heartbeatConfig;
    protected MetricConfiguration metricConfig;
    protected HealthConfiguration healthConfig;
    protected JsonObject componentConfig;
    protected JsonObject globalConfig;
    protected LoggingConfiguration loggingConfig;
    protected HashMap<String, JsonObject> instanceConfigs;
    /**
     * Whether UNS topics carry the first hierarchy value ({@code site}) after the {@code ecv1}
     * root — the top-level {@code topic.includeRoot} setting (UNS-CANONICAL-DESIGN §2.2 rule 6 /
     * D-U11), default {@code false}. Parsed by {@link #applyConfig} (so a hot reload refreshes
     * it) and read via {@link #isTopicIncludeRoot()}. Effective in {@code Uns} only with a
     * multi-level hierarchy (D-U25) — a single-level hierarchy makes it a no-op (WARN once).
     */
    protected boolean topicIncludeRoot;

    /** One-shot flag for the D-U25 includeRoot-with-single-level-hierarchy config WARN. */
    private boolean warnedIncludeRootSingleLevel = false;
    /**
     * The default {@code request()} deadline in seconds — {@code messaging.requestTimeoutSeconds}
     * (UNS-CANONICAL-DESIGN §5 / D-U5), default {@value #DEFAULT_REQUEST_TIMEOUT_SECONDS};
     * {@code 0} disables the default deadline. Parsed by {@link #applyConfig}; read via
     * {@link #getMessagingRequestTimeout()}. Late-bound onto the messaging client by
     * {@code EdgeCommons} right after this manager is constructed (§1.5 init order).
     */
    protected double messagingRequestTimeoutSeconds = DEFAULT_REQUEST_TIMEOUT_SECONDS;

    /** The schema default for {@code messaging.requestTimeoutSeconds} (seconds). */
    public static final int DEFAULT_REQUEST_TIMEOUT_SECONDS = 30;


    /**
     * Package-private constructor used by ConfigManagerFactory.
     * Use ConfigManagerFactory.create() instead of calling this directly.
     */
    /**
     * Protected no-arg constructor for testing/subclassing (e.g. mock configuration services).
     * Leaves the final identity fields null; subclasses are expected to override the accessors.
     */
    protected ConfigManager() {
        this.componentFullName = null;
        this.componentName = null;
        this.thingName = null;
        this.fullConfig = null;
        this.platform = null;
        this.componentIdentity = null;
        this.layeredConfigCoordinator = null;
    }

    ConfigManager(String componentFullName, String componentName, String thingName,
                 ConfigProvider configProvider, JsonObject fullConfig) {
        this(componentFullName, componentName, thingName, configProvider, fullConfig, null);
    }

    ConfigManager(String componentFullName, String componentName, String thingName,
                 ConfigProvider configProvider, JsonObject fullConfig, Platform platform) {
        this(componentFullName, componentName, thingName, configProvider, fullConfig, platform, null);
    }

    ConfigManager(String componentFullName, String componentName, String thingName,
                 ConfigProvider configProvider, JsonObject fullConfig, Platform platform,
                 LayeredConfigCoordinator layeredConfigCoordinator) {
        this.componentFullName = componentFullName;
        this.componentName = componentName;
        this.thingName = thingName;
        this.configProvider = configProvider;
        this.fullConfig = fullConfig;
        this.platform = platform;
        this.layeredConfigCoordinator = layeredConfigCoordinator;

        applyConfig(fullConfig);

        // Resolve the component's UNS identity ONCE, from this component's own config
        // (top-level `hierarchy` + `identity`), fail-fast on any inconsistency.
        this.componentIdentity = resolveComponentIdentity();

        // Register logging configuration change listener
        addConfigChangeListener(new LoggingConfigChangeListener(this));

        // Back-fill this manager onto the provider: providers are constructed during config
        // bootstrap with a null ConfigManager (ConfigManagerFactory builds this manager FROM the
        // provider's loaded config), so their hot-reload/push paths (applyConfig) need the
        // reference attached here. Fixes the CONFIG_COMPONENT set-config push (and the file/
        // ConfigMap watcher reload) dereferencing a forever-null parentConfigManager.
        if (configProvider != null) {
            configProvider.attachConfigManager(this);
        }
        if (layeredConfigCoordinator != null) {
            layeredConfigCoordinator.attachConfigManager(this);
        }

        // Note: initializing flag will be set to false by EdgeCommons after all initialization is complete
    }

    /**
     * Applies a new configuration to the component and notifies all registered listeners.
     *
     * @param config The new configuration to apply as a JsonObject
     */
    public void applyConfig(JsonObject config)
    {
        // On a hot reload, re-validate against the schema and keep the previous configuration if
        // the new document is invalid (parity with Python/Rust/TS, which all reject-and-keep on a
        // bad reload). Startup config is already validated by ConfigManagerFactory before the first
        // applyConfig (initializing == true), so we only re-validate subsequent reloads.
        if (!initializing) {
            try {
                ConfigurationValidator.validate(config);
            } catch (ConfigurationValidator.ConfigurationValidationException e) {
                LOGGER.error("Rejected hot-reloaded configuration (keeping previous): {}",
                        e.getMessage());
                return;
            }
        }

        // Keep the full-config snapshot current: getFullConfig() (the effective-config publisher,
        // the get-configuration verb, the opt-in subsystem inits) must reflect the APPLIED
        // configuration, not the startup snapshot, after a hot reload / push / reload-config.
        this.fullConfig = config;

        tagConfig = ConfigurationFactory.createTagConfiguration(config);
        loggingConfig = ConfigurationFactory.createLoggingConfiguration(config);
        heartbeatConfig = ConfigurationFactory.createHeartbeatConfiguration(config);
        metricConfig = ConfigurationFactory.createMetricConfiguration(config);
        healthConfig = ConfigurationFactory.createHealthConfiguration(config);
        topicIncludeRoot = parseTopicIncludeRoot(config);
        // D-U25: includeRoot needs a level ABOVE the device to prepend — with a single-level
        // hierarchy (the zero-config ["device"] default) hier[0] IS the device, so the setting
        // is a no-op in Uns (prepending would duplicate the device). Tell the user once.
        if (topicIncludeRoot && !warnedIncludeRootSingleLevel && hierarchyLevelCount(config) == 1)
        {
            warnedIncludeRootSingleLevel = true;
            LOGGER.warn("topic.includeRoot=true has no effect with a single-level hierarchy"
                    + " (hierarchy.levels resolves to one level - the device): the site position"
                    + " requires a level above the device, so UNS topics stay rootless."
                    + " Declare a multi-level hierarchy.levels or remove topic.includeRoot.");
        }
        messagingRequestTimeoutSeconds = parseMessagingRequestTimeoutSeconds(config);
        reconfigureLogging();

        componentConfig = config.get("component").getAsJsonObject();
        globalConfig = componentConfig.has("global")
                ? componentConfig.get("global").getAsJsonObject()
                : new JsonObject();
        genInstancesMap();
        if (!initializing) {
            notifyConfigurationChanged();
        }
    }

    /**
     * Re-fetches the configuration from the active config source and re-applies it — the
     * {@code reload-config} command verb's action (DESIGN-uns §9.5). Re-invokes the provider's
     * {@code loadConfiguration()} (re-reads the file / ConfigMap / env / shadow / GG config, or
     * re-requests from the config component), validates the document against the schema, and
     * applies it via {@link #applyConfig} (which notifies the change listeners, so a successful
     * reload also re-announces the {@code cfg} push). Best-effort: any failure is logged and
     * reported as {@code false} — a reload must never crash a running component.
     *
     * @return {@code true} when a document was fetched, validated and applied; {@code false}
     *         when no provider is wired (test/subclass bring-up), the fetch failed/returned
     *         nothing, or the document was schema-invalid (the previous config is kept)
     */
    public boolean reloadFromProvider()
    {
        if (configProvider == null)
        {
            LOGGER.warn("reload-config requested but no config provider is wired - ignoring");
            return false;
        }
        JsonObject newConfig;
        try
        {
            newConfig = layeredConfigCoordinator != null
                    ? layeredConfigCoordinator.reloadEffectiveFromProvider()
                    : configProvider.loadConfiguration();
        }
        catch (Exception e)
        {
            LOGGER.warn("reload-config: re-fetch from the '{}' source failed: {}",
                    configProvider.getConfigSource(), e.toString());
            return false;
        }
        if (newConfig == null)
        {
            LOGGER.warn("reload-config: the '{}' source returned no configuration - keeping the"
                    + " previous configuration", configProvider.getConfigSource());
            return false;
        }
        try
        {
            // Validate BEFORE applying so the caller gets a truthful verdict (applyConfig's own
            // reject-and-keep path logs but does not report).
            ConfigurationValidator.validate(newConfig);
        }
        catch (ConfigurationValidator.ConfigurationValidationException e)
        {
            LOGGER.error("reload-config: rejected re-fetched configuration (keeping previous): {}",
                    e.getMessage());
            return false;
        }
        applyConfig(newConfig);
        LOGGER.info("reload-config: configuration re-fetched and re-applied from the '{}' source",
                configProvider.getConfigSource());
        return true;
    }

    /**
     * Applies a raw provider payload (component document or CONFIG_COMPONENT bundle) by resolving
     * split-config layers first. Used by provider push/watch callbacks; returns false and keeps the
     * current effective config on any merge/validation failure.
     */
    public boolean applyConfigFromProvider(JsonObject rawConfig)
    {
        if (layeredConfigCoordinator == null)
        {
            applyConfig(rawConfig);
            return true;
        }
        try
        {
            JsonObject effective = layeredConfigCoordinator.applyProviderPayload(rawConfig);
            applyConfig(effective);
            return true;
        }
        catch (Exception e)
        {
            LOGGER.warn("Rejected provider configuration update (keeping previous): {}",
                    e.getMessage(), e);
            return false;
        }
    }


    /**
     * Parses the top-level {@code topic.includeRoot} flag (default {@code false}). Minimal
     * config-model support for the {@code topic} section — lenient like the other permissive
     * subsystem sections: a missing/non-object {@code topic} or a missing/non-boolean
     * {@code includeRoot} yields the default.
     */
    private static boolean parseTopicIncludeRoot(JsonObject config)
    {
        if (config == null || !config.has("topic") || !config.get("topic").isJsonObject())
        {
            return false;
        }
        JsonElement includeRoot = config.getAsJsonObject("topic").get("includeRoot");
        return includeRoot != null && includeRoot.isJsonPrimitive()
                && includeRoot.getAsJsonPrimitive().isBoolean() && includeRoot.getAsBoolean();
    }

    /**
     * Whether UNS topics built by {@code gg.getUns()} / {@code gg.instance(id).uns()} carry the
     * first hierarchy value ({@code site}) between the {@code ecv1} root and the device — the
     * top-level {@code topic.includeRoot} setting, default {@code false}. Note that {@code Uns}
     * applies it only when the hierarchy is multi-level (D-U25).
     *
     * @return the resolved {@code topic.includeRoot} value
     */
    public boolean isTopicIncludeRoot()
    {
        return topicIncludeRoot;
    }

    /**
     * Lenient {@code hierarchy.levels} entry count for the D-U25 config WARN: a missing/malformed
     * {@code hierarchy} section counts as the zero-config single-level default ({@code ["device"]}).
     * Strict validation of the section happens in {@code resolveComponentIdentity} (fail-fast at
     * construction); this helper must never throw on shapes the WARN check sees first.
     */
    private static int hierarchyLevelCount(JsonObject config)
    {
        if (config == null || !config.has("hierarchy") || !config.get("hierarchy").isJsonObject())
        {
            return 1;
        }
        JsonElement levels = config.getAsJsonObject("hierarchy").get("levels");
        if (levels == null || !levels.isJsonArray() || levels.getAsJsonArray().isEmpty())
        {
            return 1;
        }
        return levels.getAsJsonArray().size();
    }

    /**
     * Parses {@code messaging.requestTimeoutSeconds} (§5 / D-U5): a non-negative number of seconds
     * (fractions allowed by the schema), default {@value #DEFAULT_REQUEST_TIMEOUT_SECONDS}.
     * Lenient like the other permissive sections — a missing/non-object {@code messaging} section,
     * a missing/non-number value, or a negative value (which the schema rejects at startup anyway)
     * all yield the default. {@code 0} is a valid explicit value meaning "disabled".
     */
    private static double parseMessagingRequestTimeoutSeconds(JsonObject config)
    {
        if (config == null || !config.has("messaging") || !config.get("messaging").isJsonObject())
        {
            return DEFAULT_REQUEST_TIMEOUT_SECONDS;
        }
        JsonElement value = config.getAsJsonObject("messaging").get("requestTimeoutSeconds");
        if (value == null || !value.isJsonPrimitive() || !value.getAsJsonPrimitive().isNumber())
        {
            return DEFAULT_REQUEST_TIMEOUT_SECONDS;
        }
        double seconds = value.getAsDouble();
        return seconds < 0 ? DEFAULT_REQUEST_TIMEOUT_SECONDS : seconds;
    }

    /**
     * The default {@code request()} deadline resolved from {@code messaging.requestTimeoutSeconds}
     * (UNS-CANONICAL-DESIGN §5 / D-U5), default 30 s. Returns {@link java.time.Duration#ZERO} when
     * the configured value is {@code 0} (default deadline disabled). {@code EdgeCommons} late-binds
     * this onto the messaging client right after this manager is constructed; an explicit per-call
     * timeout on {@code request()} always wins over this default.
     *
     * @return the default request deadline ({@code Duration.ZERO} = disabled)
     */
    public java.time.Duration getMessagingRequestTimeout()
    {
        return messagingRequestTimeoutSeconds <= 0
                ? java.time.Duration.ZERO
                : java.time.Duration.ofMillis(Math.round(messagingRequestTimeoutSeconds * 1000.0));
    }

    /**
     * Generates a map of instance configurations from the full configuration.
     * This is an internal method used to organize instance-specific settings.
     */
    private void genInstancesMap()
    {
        instanceConfigs = new HashMap<>();
        JsonArray instances = componentConfig.has("instances")
                ? componentConfig.get("instances").getAsJsonArray()
                : null;
        if (instances != null)
        {
            for (JsonElement instance : instances)
            {
                JsonObject instanceConfig = instance.getAsJsonObject();
                instanceConfigs.put(instanceConfig.get("id").getAsString(), instanceConfig);
                LOGGER.debug("Loaded instance config for {}", instanceConfig.get("id"));
            }
        }
    }


    /**
     * Returns the global configuration that applies to all instances.
     *
     * @return JsonObject containing global configuration settings
     */
    public JsonObject getGlobalConfig()
    {
        return globalConfig;
    }

    /**
     * Returns the collection of all instance IDs defined in the configuration.
     *
     * @return Collection of instance identifier strings
     */
    public Collection<String> getInstanceIds()
    {
        return instanceConfigs.keySet();
    }

    /**
     * Returns the configuration for a specific instance.
     *
     * @param instanceId The identifier of the instance
     * @return JsonObject containing instance-specific configuration
     */
    public JsonObject getInstanceConfig(String instanceId)
    {
        return instanceConfigs.getOrDefault(instanceId, null);
    }

    /**
     * Returns the complete configuration including global and instance-specific settings.
     *
     * @return JsonObject containing the full configuration
     */
    public JsonObject getFullConfig() { return fullConfig; }

    /**
     * Returns the tag configuration settings.
     *
     * @return TagConfiguration object containing tag-related settings
     */
    public TagConfiguration getTagConfig()
    {
        return tagConfig;
    }

    /**
     * Returns the heartbeat configuration settings.
     *
     * @return HeartbeatConfiguration object containing heartbeat-related settings
     */
    public HeartbeatConfiguration getHeartbeatConfig()
    {
        return heartbeatConfig;
    }

    /**
     * Returns the logging configuration settings.
     *
     * @return LoggingConfiguration object containing logging-related settings
     */
    public LoggingConfiguration getLoggingConfig()
    {
        return loggingConfig;
    }

    /**
     * Returns the metric configuration settings.
     *
     * @return MetricConfiguration object containing metric-related settings
     */
    public MetricConfiguration getMetricConfig() {
        return metricConfig;
    }

    /**
     * Returns the resolved deployment {@link Platform}, or {@code null} when unknown (e.g. test /
     * subclass bring-up). Lets subsystem targets apply platform-profile defaults — e.g. the metric
     * {@code log} target's HOST-aware log-file path (a local path off-device rather than the
     * GREENGRASS {@code /greengrass/v2/logs} default); see
     * {@link com.mbreissi.edgecommons.platform.PlatformResolver#profileMetricLogPath(Platform)}.
     *
     * @return the resolved platform, or {@code null} if not threaded in
     */
    public Platform getPlatform() {
        return platform;
    }

    /**
     * Returns the HTTP health-endpoint configuration (the {@code health} config section, or defaults
     * when absent). Drives the Kubernetes liveness/readiness/startup probe server (FR-HB-1). Whether
     * the server actually starts is decided by {@link com.mbreissi.edgecommons.EdgeCommons} from the
     * explicit {@code health.enabled} ▸ the KUBERNETES platform default ▸ {@code false} precedence.
     *
     * @return HealthConfiguration object containing health-endpoint settings
     */
    public HealthConfiguration getHealthConfig() {
        return healthConfig;
    }

    /**
     * Returns the name of the AWS IoT thing associated with this component.
     *
     * @return The thing name or null if not available
     */
    public String getThingName()
    {
        return thingName;
    }

    /**
     * Returns the component's resolved UNS identity (instance
     * {@value MessageIdentity#DEFAULT_INSTANCE}), resolved once at construction from the
     * component's OWN config:
     * <ol>
     *   <li>{@code levels} = top-level {@code hierarchy.levels} when present, else the
     *       zero-config default {@code ["device"]}.</li>
     *   <li>Level names must match {@code ^[A-Za-z0-9_-]+$}, be unique and non-empty.</li>
     *   <li>Every level except the last takes its value from the top-level {@code identity}
     *       config object (a missing value is a startup error naming the level); the LAST
     *       level's value is the resolved thing name (the existing identity chain).</li>
     *   <li>An {@code identity} key equal to the last level name, or not among the declared
     *       non-device levels, is a startup error (typo protection the schema cannot express).</li>
     *   <li>Every value and the component short name pass through the template sanitizer.</li>
     * </ol>
     *
     * @return the resolved identity, or {@code null} only on the protected test/subclass
     *         bring-up constructor (which resolves no config)
     */
    public MessageIdentity getComponentIdentity()
    {
        return componentIdentity;
    }

    /** Strict UNS hierarchy level-name rule (future Parquet columns — keep it tight). */
    private static final Pattern HIERARCHY_LEVEL_NAME = Pattern.compile("^[A-Za-z0-9_-]+$");

    /**
     * Resolves the component identity from the already-applied {@link #fullConfig} (see
     * {@link #getComponentIdentity()} for the algorithm). Called once from the constructor;
     * fail-fast with a precise {@link IllegalStateException} (wrapped into a
     * {@code ConfigurationException} by {@link ConfigManagerFactory}).
     */
    private MessageIdentity resolveComponentIdentity()
    {
        // 1. levels = hierarchy.levels if present, else the zero-config default ["device"].
        List<String> levels = new ArrayList<>();
        if (fullConfig != null && fullConfig.has("hierarchy"))
        {
            JsonElement hierarchyEl = fullConfig.get("hierarchy");
            if (!hierarchyEl.isJsonObject() || !hierarchyEl.getAsJsonObject().has("levels"))
            {
                throw identityError("'hierarchy' must be an object with a 'levels' array");
            }
            JsonElement levelsEl = hierarchyEl.getAsJsonObject().get("levels");
            if (!levelsEl.isJsonArray() || levelsEl.getAsJsonArray().isEmpty())
            {
                throw identityError("'hierarchy.levels' must be a non-empty array of level names");
            }
            for (JsonElement levelEl : levelsEl.getAsJsonArray())
            {
                if (!levelEl.isJsonPrimitive() || !levelEl.getAsJsonPrimitive().isString())
                {
                    throw identityError("'hierarchy.levels' entries must be strings");
                }
                levels.add(levelEl.getAsString());
            }
        }
        else
        {
            levels.add("device");
        }

        // 2. Level names: strict charset, unique, non-empty.
        Set<String> seen = new LinkedHashSet<>();
        for (String level : levels)
        {
            if (level == null || !HIERARCHY_LEVEL_NAME.matcher(level).matches())
            {
                throw identityError("invalid hierarchy level name '" + level
                        + "' (must match ^[A-Za-z0-9_-]+$)");
            }
            if (!seen.add(level))
            {
                throw identityError("duplicate hierarchy level name '" + level + "'");
            }
        }
        String deviceLevel = levels.get(levels.size() - 1);
        List<String> valueLevels = levels.subList(0, levels.size() - 1);

        // 3/4. The `identity` config object supplies every level's value except the last;
        //      keys must be exactly (a subset of) the non-device levels.
        JsonObject identityConfig = new JsonObject();
        if (fullConfig != null && fullConfig.has("identity"))
        {
            JsonElement identityEl = fullConfig.get("identity");
            if (!identityEl.isJsonObject())
            {
                throw identityError("'identity' must be an object of level-name -> value");
            }
            identityConfig = identityEl.getAsJsonObject();
        }
        for (String key : identityConfig.keySet())
        {
            if (key.equals(deviceLevel))
            {
                throw identityError("'identity." + key + "' must not be set: '" + deviceLevel
                        + "' is the last hierarchy level (the device) and its value is always the"
                        + " resolved thing name");
            }
            if (!valueLevels.contains(key))
            {
                throw identityError("'identity." + key + "' is not a declared hierarchy level;"
                        + " expected keys: " + valueLevels);
            }
        }

        List<MessageIdentity.HierEntry> hier = new ArrayList<>();
        List<String> missing = new ArrayList<>();
        for (String level : valueLevels)
        {
            JsonElement valueEl = identityConfig.get(level);
            if (valueEl == null || !valueEl.isJsonPrimitive() || valueEl.getAsString().isEmpty())
            {
                missing.add(level);
                continue;
            }
            hier.add(new MessageIdentity.HierEntry(level, sanitizedIdentityValue(level, valueEl.getAsString())));
        }
        if (!missing.isEmpty())
        {
            throw identityError("the top-level 'identity' config object is missing value(s) for"
                    + " hierarchy level(s) " + missing + " (hierarchy.levels = " + levels
                    + "; the last level '" + deviceLevel + "' is the resolved thing name and must"
                    + " not be configured)");
        }

        // The device (last level) value is the resolved thing name (PlatformResolver chain).
        if (thingName == null || thingName.isEmpty())
        {
            throw identityError("the device level '" + deviceLevel + "' value (the resolved thing"
                    + " name) is not available");
        }
        hier.add(new MessageIdentity.HierEntry(deviceLevel, sanitizedIdentityValue(deviceLevel, thingName)));

        // 5. component = explicit token when configured, else sanitized short name.
        if (componentName == null || componentName.isEmpty())
        {
            throw identityError("the component name is not available");
        }
        String configuredToken = configuredComponentToken();
        String componentToken = sanitizedIdentityValue("component",
                configuredToken != null ? configuredToken : componentName);
        return new MessageIdentity(hier, componentToken, MessageIdentity.DEFAULT_INSTANCE);
    }

    /** Returns {@code component.token} when configured. */
    private String configuredComponentToken()
    {
        if (fullConfig == null || !fullConfig.has("component"))
        {
            return null;
        }
        JsonElement componentEl = fullConfig.get("component");
        if (componentEl == null || componentEl.isJsonNull())
        {
            return null;
        }
        if (!componentEl.isJsonObject())
        {
            throw identityError("'component' must be an object when configuring 'component.token'");
        }
        JsonObject component = componentEl.getAsJsonObject();
        if (!component.has("token"))
        {
            return null;
        }
        JsonElement tokenEl = component.get("token");
        if (tokenEl == null || !tokenEl.isJsonPrimitive() || tokenEl.getAsString().isEmpty())
        {
            throw identityError("'component.token' must be a non-empty string");
        }
        return tokenEl.getAsString();
    }

    /** Sanitizes an identity value via the template sanitizer, WARN-logging when it changed. */
    private static String sanitizedIdentityValue(String what, String rawValue)
    {
        String sanitized = sanitize(rawValue);
        if (!sanitized.equals(rawValue))
        {
            LOGGER.warn("Identity value for '{}' contained reserved characters and was sanitized:"
                    + " '{}' -> '{}'", what, rawValue, sanitized);
        }
        return sanitized;
    }

    /** Builds the uniform fail-fast identity-resolution startup error. */
    private static IllegalStateException identityError(String detail)
    {
        return new IllegalStateException("Component identity resolution failed: " + detail);
    }

    /**
     * Returns the short name of this component.
     *
     * @return The component name
     */
    public String getComponentName()
    {
        return componentName;
    }

    /**
     * Returns the full qualified name of this component.
     *
     * @return The fully qualified component name
     */
    public String getComponentFullName()
    {
        return componentFullName;
    }

    /**
     * Adds a listener to be notified of configuration changes.
     *
     * @param listener The listener to add
     */
    public void addConfigChangeListener(ConfigurationChangeListener listener)
    {
        configChangeListeners.add(listener);
    }

    /**
     * Removes a previously added configuration change listener.
     *
     * @param listener The listener to remove
     */
    public void removeConfigChangeListener(ConfigurationChangeListener listener)
    {
        configChangeListeners.remove(listener);
    }

    /**
     * Notifies all registered configuration change listeners of a configuration change.
     * This should only be called for actual runtime configuration changes, not during initialization.
     */
    public void notifyConfigurationChanged()
    {
        LOGGER.info("configurationChanged: Notifying {} listeners", configChangeListeners.size());
        for (ConfigurationChangeListener listener : configChangeListeners)
        {
            if (listener == null) {
                LOGGER.error("ConfigurationChangeListener is null.  Not notifying.");
                continue;
            }
            // Isolate each listener: one listener throwing must not prevent the others from being notified.
            try {
                listener.onConfigurationChanged();
            } catch (Exception e) {
                LOGGER.error("ConfigurationChangeListener {} threw during notification: {}",
                        listener.getClass().getName(), e.getMessage(), e);
            }
        }
    }
    
    /**
     * Marks initialization as complete. Called by EdgeCommons after all initialization is finished.
     * Future configuration changes will trigger listener notifications.
     */
    public void close()
    {
        if (layeredConfigCoordinator != null) {
            layeredConfigCoordinator.close();
        }
        if (configProvider != null) {
            configProvider.close();
        }
    }

    public void completeInitialization()
    {
        initializing = false;
        LOGGER.debug("ConfigManager initialization completed - listeners will now be notified of configuration changes");
    }


    /**
     * Resolves a template string by replacing placeholders with actual values.
     * Supports component name, thing name, and other configuration-based substitutions.
     *
     * @param template The template string containing placeholders
     * @return The resolved string with substituted values
     */
    public String resolveTemplate(String template) {
        String retVal = template;
        if (template.contains("{ThingName}"))
        {
            retVal = retVal.replace("{ThingName}", sanitize(getThingName()));
        }
        if (template.contains("{ComponentName}"))
        {
            retVal = retVal.replace("{ComponentName}", sanitize(getComponentName()));
        }
        if (template.contains("{ComponentFullName}"))
        {
            retVal = retVal.replace("{ComponentFullName}", sanitize(getComponentFullName()));
        }

        if (null != tagConfig && tagConfig.getKeys() != null)
        {
            for (String tagKey : tagConfig.getKeys())
            {
                String hierarchyLevelTemplate = "{" + tagKey + "}";
                if (retVal.contains(hierarchyLevelTemplate))
                {
                    retVal = retVal.replace(hierarchyLevelTemplate, sanitize(tagConfig.getKeyValue(tagKey)));
                }
            }
        }

        return retVal;
    }

    /**
     * Neutralizes characters in a substituted value that are dangerous in a file
     * path or MQTT topic: path separators ({@code /}, {@code \}), traversal dot
     * sequences ({@code ..}), MQTT wildcards ({@code +}, {@code #}), and control
     * characters are each replaced with {@code _}. Applied only to interpolated
     * values, never to the surrounding template, so structural separators in the
     * template are preserved. Mirrors the Rust library's {@code config::template::sanitize}.
     *
     * <p>Public because it is also the normative UNS channel-token sanitizer (UNS-CANONICAL-DESIGN
     * §2.2 rule 1 / D-U26): the {@code uns()} token rule is exactly this blacklist, so
     * "sanitized ⇒ publishable" holds. The metric {@code Messaging} target uses it to turn a
     * metric name into the {@code metric/{metricName}} channel token (§4.3).
     *
     * @param value The raw value to be interpolated (may be null)
     * @return The sanitized value, or an empty string if {@code value} is null
     */
    public static String sanitize(String value)
    {
        if (value == null)
        {
            return "";
        }
        StringBuilder sb = new StringBuilder(value.length());
        for (int i = 0; i < value.length(); i++)
        {
            char c = value.charAt(i);
            if (c == '/' || c == '\\' || c == '+' || c == '#' || Character.isISOControl(c))
            {
                sb.append('_');
            }
            else
            {
                sb.append(c);
            }
        }
        // Collapse traversal sequences (e.g. "..") that remain after separator replacement.
        return sb.toString().replace("..", "_");
    }

    /**
     * Reconfigures the logging system based on the current logging configuration.
     * Can operate in global mode (controls entire app) or isolated mode (EdgeCommons only).
     */
    public void reconfigureLogging()
    {
        // Check if global logging control is enabled
        boolean globalControl = getLoggingConfig().toDict().has("globalControl") && 
                               getLoggingConfig().toDict().get("globalControl").getAsBoolean();
        
        if (globalControl) {
            new GlobalLoggingManager(this, true).configureGlobalLogging();
            return;
        }
        
        try {
            // Keep the old implementation commented out for reference
//        ConfigurationBuilder<BuiltConfiguration> configBuilder = newConfigurationBuilder();
//
//        AppenderComponentBuilder consoleAppenderBuilder = configBuilder.newAppender("stdout", "Console");
//        configBuilder.add(consoleAppenderBuilder);
//
//
//        LayoutComponentBuilder layoutComponentBuilder = configBuilder.newLayout("PatternLayout");
//        layoutComponentBuilder.addAttribute("pattern", getLoggingConfig().getFormat());
//
//        consoleAppenderBuilder.addComponent(layoutComponentBuilder);
//
//        configBuilder.add(consoleAppenderBuilder);
//
//        RootLoggerComponentBuilder rootLogger = configBuilder.newRootLogger(getLoggingConfig().getLevel());
//        rootLogger.add(configBuilder.newAppenderRef("stdout"));
//        configBuilder.add(rootLogger);
//
//        Configurator.reconfigure(configBuilder.build());
//        Configurator.setAllLevels(LogManager.getRootLogger().getName(), getLoggingConfig().getLevel());

            // Get the current logger context
            LoggerContext context = (LoggerContext) LogManager.getContext(false);
            
            // Create a new configuration builder
            ConfigurationBuilder<BuiltConfiguration> builder = newConfigurationBuilder();
            
            // Set basic configuration properties
            builder.setStatusLevel(Level.INFO);
            builder.setConfigurationName("DynamicConfig-" + componentName);

            // FR-LOG-1/4 (precedence FR-RT-3): resolve the effective logging-format token —
            // explicit `logging.java_format` ▸ platform-profile default (`json` on KUBERNETES) ▸ the
            // library default. The `json` token (case-insensitive) selects the structured stdout-JSON
            // sink; any other token is a Log4j2 PatternLayout pattern (behavior unchanged).
            String effectiveFormat = resolveEffectiveLogFormat();
            boolean jsonSink = PlatformResolver.LOGGING_FORMAT_JSON.equalsIgnoreCase(effectiveFormat);

            // FR-LOG-2: the stdout-JSON sink (the KUBERNETES default) is stdout-only — no in-process
            // size-rotation is installed (the cluster log agent owns rotation), so a read-only root FS
            // never breaks logging. Off the JSON sink the optional RollingFile appender is unchanged.
            // Parity with Python/Rust, which also drop the file appender under the JSON sink.
            boolean fileLogging = !jsonSink
                    && getLoggingConfig().isFileLoggingEnabled()
                    && getLoggingConfig().getLogFilePath() != null;

            // The console (SYSTEM_OUT) appender is always installed; only its LAYOUT changes. The
            // JSON layout is a PatternLayout built from Log4j2 built-in converters (%enc{..}{JSON}
            // escaping + %notEmpty conditional fields) that emits one valid JSON object per line — no
            // extra dependency / Log4j2 plugin, so the shaded self-contained JAR keeps working.
            String pattern = jsonSink ? buildJsonPattern(correlationFields()) : effectiveFormat;

            // Create the console appender with the resolved layout
            LayoutComponentBuilder layoutBuilder = builder.newLayout("PatternLayout")
                .addAttribute("pattern", pattern);
            if (jsonSink) {
                // The JSON pattern renders the exception itself (a single escaped line under "thrown");
                // disable PatternLayout's automatic trailing throwable append, which would otherwise
                // emit a raw multi-line stack trace AFTER the JSON object and break one-object-per-line.
                layoutBuilder.addAttribute("alwaysWriteExceptions", false);
            }

            AppenderComponentBuilder consoleAppender = builder.newAppender("Console", "Console")
                .addAttribute("target", ConsoleAppender.Target.SYSTEM_OUT)
                .add(layoutBuilder);

            builder.add(consoleAppender);

            // Create a file appender if file logging is enabled (never under the JSON sink)
            if (fileLogging) {
                String logFilePath = getLoggingConfig().getLogFilePath();
                
                // Resolve any template variables in the file path
                logFilePath = resolveTemplate(logFilePath);

                // Size-rotated file output: rotate at maxFileSize, keep backupCount
                // backups named <path>.1, <path>.2 (fileIndex=min => .1 is newest),
                // matching the Python/Rust libraries' RotatingFileHandler contract.
                AppenderComponentBuilder fileAppender = builder.newAppender("File", "RollingFile")
                    .addAttribute("fileName", logFilePath)
                    .addAttribute("filePattern", logFilePath + ".%i")
                    .addAttribute("append", true)
                    .add(layoutBuilder)
                    .addComponent(builder.newComponent("Policies")
                        .addComponent(builder.newComponent("SizeBasedTriggeringPolicy")
                            .addAttribute("size", getLoggingConfig().getMaxFileSize())))
                    .addComponent(builder.newComponent("DefaultRolloverStrategy")
                        .addAttribute("max", getLoggingConfig().getBackupCount())
                        .addAttribute("fileIndex", "min"));

                builder.add(fileAppender);
            }
            
            // Configure the root logger with the specified level
            Level rootLevel = getLoggingConfig().getLevel();
            RootLoggerComponentBuilder rootLogger = builder.newRootLogger(rootLevel);
            rootLogger.add(builder.newAppenderRef("Console"));

            // Add file appender reference to root logger if file logging is enabled
            if (fileLogging) {
                rootLogger.add(builder.newAppenderRef("File"));
            }
            
            builder.add(rootLogger);
            
            // Configure specific loggers if defined
            Map<String, Level> loggerLevels = getLoggingConfig().getLoggerLevels();
            for (Map.Entry<String, Level> entry : loggerLevels.entrySet()) {
                String loggerName = entry.getKey();
                Level level = entry.getValue();
                
                LoggerComponentBuilder loggerBuilder = builder.newLogger(loggerName, level)
                    .add(builder.newAppenderRef("Console"));
                
                // Add file appender reference if file logging is enabled
                if (fileLogging) {
                    loggerBuilder.add(builder.newAppenderRef("File"));
                }
                
                // Set additivity to false to prevent duplicate logging
                loggerBuilder.addAttribute("additivity", false);
                
                builder.add(loggerBuilder);
            }
            
            // Build the new configuration
            Configuration newConfig = builder.build();
            
            // Apply the new configuration
            context.start(newConfig);
            context.updateLoggers();
            
            LOGGER.info("Logging reconfigured with root level: {} and format: {} (sink: {})",
                      rootLevel,
                      effectiveFormat,
                      jsonSink ? "stdout-JSON" : "console/text");
            
            // Log information about configured loggers
            if (!loggerLevels.isEmpty()) {
                LOGGER.info("Configured {} specific logger levels", loggerLevels.size());
                for (Map.Entry<String, Level> entry : loggerLevels.entrySet()) {
                    LOGGER.debug("Logger '{}' configured with level: {}", entry.getKey(), entry.getValue());
                }
            }
            
            // Log information about file logging
            if (fileLogging) {
                LOGGER.info("File logging enabled with path: {}", resolveTemplate(getLoggingConfig().getLogFilePath()));
            }
            
        } catch (Exception e) {
            // If reconfiguration fails, log the error but don't crash the application
            LOGGER.error("Failed to reconfigure logging: {}", e.getMessage(), e);
            LOGGER.warn("Continuing with previous logging configuration");
        }
    }

    /**
     * Resolves the <em>effective</em> logging-format token under the FR-RT-3 precedence
     * (FR-LOG-1/4): an explicit {@code logging.java_format} from the component config ▸ the
     * platform-profile default ({@value PlatformResolver#LOGGING_FORMAT_JSON} on
     * {@link Platform#KUBERNETES}, via {@link PlatformResolver#profileLoggingFormat}) ▸ the library
     * default ({@link LoggingConfiguration#DEFAULT_FORMAT}). The resolved platform is known before the
     * component config loads, so a pod with no {@code logging.java_format} logs JSON, while setting it
     * to a non-{@code json} value overrides; GREENGRASS/HOST (no profile default) keep today's default.
     *
     * @return the effective logging-format token (never {@code null})
     */
    String resolveEffectiveLogFormat() {
        LoggingConfiguration cfg = getLoggingConfig();
        if (cfg.isFormatExplicitlySet()) {
            return cfg.getFormat();  // explicit config wins (FR-RT-3 top tier)
        }
        String profileDefault = PlatformResolver.profileLoggingFormat(platform);
        if (profileDefault != null && !profileDefault.isEmpty()) {
            return profileDefault;  // platform-profile default (json on KUBERNETES)
        }
        return cfg.getFormat();  // library default
    }

    /**
     * Builds the best-effort logging correlation fields (FR-LOG-3) for the stdout-JSON sink:
     * {@code thing} (the resolved identity, when non-empty) plus {@code pod}/{@code namespace}/
     * {@code node} from the KUBERNETES Downward-API env vars ({@link PlatformResolver#ENV_K8S_POD_NAME}
     * / {@link PlatformResolver#ENV_K8S_POD_NAMESPACE} / {@link PlatformResolver#ENV_K8S_NODE_NAME} —
     * the same vars wired in Phase 1b). Absent / empty values are omitted (never emitted as empty/null
     * noise). Captured once at (re)configuration time; insertion order is preserved for stable output.
     * Mirrors Rust {@code correlation_fields} and Python {@code _logging_correlation}.
     *
     * @return an ordered map of correlation field name to value (possibly empty)
     */
    // Package-private so {@link GlobalLoggingManager} (the logging.globalControl=true path) can build
    // the same correlation fields as {@link #reconfigureLogging()}.
    Map<String, String> correlationFields() {
        Map<String, String> fields = new LinkedHashMap<>();
        String thing = getThingName();
        if (thing != null && !thing.isEmpty()) {
            fields.put("thing", thing);
        }
        putEnvField(fields, "pod", PlatformResolver.ENV_K8S_POD_NAME);
        putEnvField(fields, "namespace", PlatformResolver.ENV_K8S_POD_NAMESPACE);
        putEnvField(fields, "node", PlatformResolver.ENV_K8S_NODE_NAME);
        return fields;
    }

    /** Adds {@code key -> System.getenv(envVar)} to {@code fields} only when the env var is non-empty. */
    private static void putEnvField(Map<String, String> fields, String key, String envVar) {
        String value = System.getenv(envVar);
        if (value != null && !value.isEmpty()) {
            fields.put(key, value);
        }
    }

    /**
     * Builds the Log4j2 PatternLayout pattern for the structured stdout-JSON sink (FR-LOG-1): one
     * valid JSON object per line carrying {@code timestamp} (ISO-8601 UTC), {@code level},
     * {@code logger}, {@code message}, the supplied correlation fields, and {@code thrown} (the
     * exception/stack trace) <em>only when an exception is present</em>. The field set mirrors the
     * Python/Rust/TS sinks for four-way parity.
     *
     * <p>Validity is guaranteed by built-in converters: {@code %enc{...}{JSON}} JSON-escapes every
     * dynamic value (quotes, backslashes, control chars → escape sequences, so the object stays on one
     * physical line); the exception is first run through {@code %replace{%ex}{[\r\n\t]+}{ }} to
     * collapse its stack-trace newlines/tabs to single spaces (a Log4j2 throwable converter otherwise
     * emits raw newlines that {@code %enc} does not fold), then JSON-escaped; and {@code %notEmpty{...}}
     * omits the {@code thrown} field when {@code %ex} is empty. No extra dependency or custom Log4j2
     * plugin is needed, keeping the shaded self-contained JAR intact. <b>The owning appender's
     * PatternLayout MUST set {@code alwaysWriteExceptions=false}</b> so PatternLayout does not also
     * append a raw multi-line stack trace after the JSON object (see {@link #reconfigureLogging()}).
     *
     * <p>Correlation values are emitted as literal JSON (process-wide constants captured at configure
     * time, so they appear on every line regardless of the logging thread — MDC/ThreadContext is
     * thread-local and would miss other threads). They are first {@linkplain #sanitizeForJsonLiteral
     * sanitized} because a Log4j2 PatternLayout un-escapes backslash sequences (e.g. {@code \n}) in
     * literal text, which would otherwise undo JSON escaping and break the line.
     *
     * @param correlation the correlation fields to embed (already filtered to present values)
     * @return the JSON PatternLayout pattern
     */
    static String buildJsonPattern(Map<String, String> correlation) {
        StringBuilder p = new StringBuilder(256);
        // timestamp in UTC with a literal 'Z' (parity with the Python/Rust sinks).
        p.append("{\"timestamp\":\"%d{yyyy-MM-dd'T'HH:mm:ss.SSS'Z'}{UTC}\"")
                .append(",\"level\":\"%p\"")
                .append(",\"logger\":\"%enc{%c}{JSON}\"")
                .append(",\"message\":\"%enc{%m}{JSON}\"");
        if (correlation != null) {
            for (Map.Entry<String, String> entry : correlation.entrySet()) {
                p.append(jsonField(entry.getKey(), entry.getValue()));
            }
        }
        // The exception/stack trace is included only when present; CR/LF/TAB runs are collapsed to a
        // single space so the trace stays on one physical line, then JSON-escaped. `thrown` mirrors
        // the Python sink's key for the formatted exception.
        p.append("%notEmpty{,\"thrown\":\"%enc{%replace{%ex}{[\\r\\n\\t]+}{ }}{JSON}\"}");
        p.append("}%n");
        return p.toString();
    }

    /**
     * Renders a single constant JSON field {@code ,"key":"value"} for embedding as a literal in the
     * JSON PatternLayout pattern, or {@code ""} when the value is null/empty. The value is
     * {@linkplain #sanitizeForJsonLiteral sanitized} (never JSON-escaped) because PatternLayout
     * un-escapes backslash sequences in literal text.
     */
    static String jsonField(String key, String value) {
        if (value == null || value.isEmpty()) {
            return "";
        }
        return ",\"" + key + "\":\"" + sanitizeForJsonLiteral(value) + "\"";
    }

    /**
     * Neutralizes characters that are unsafe inside a JSON string literal embedded in a Log4j2
     * PatternLayout pattern: the double-quote and backslash (would break JSON / be un-escaped by
     * PatternLayout), the {@code %} (the PatternLayout converter sigil), and control characters (no
     * raw control chars in JSON) are each replaced with {@code _}. Best-effort: correlation values are
     * non-sensitive identifiers (pod/namespace/node/thing), so neutralizing a stray special character
     * is acceptable — mirrors the philosophy of {@link #sanitize(String)} for template values.
     */
    static String sanitizeForJsonLiteral(String value) {
        StringBuilder sb = new StringBuilder(value.length());
        for (int i = 0; i < value.length(); i++) {
            char c = value.charAt(i);
            if (c == '"' || c == '\\' || c == '%' || Character.isISOControl(c)) {
                sb.append('_');
            } else {
                sb.append(c);
            }
        }
        return sb.toString();
    }
}
