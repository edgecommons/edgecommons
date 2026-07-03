/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.config;

import com.mbreissi.ggcommons.ParsedCommandLine;
import com.mbreissi.ggcommons.config.provider.ConfigProvider;
import com.mbreissi.ggcommons.config.provider.ConfigProviderBuilder;
import com.mbreissi.ggcommons.messaging.MessageIdentity;
import com.mbreissi.ggcommons.platform.Platform;
import com.mbreissi.ggcommons.platform.PlatformResolver;

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
    protected final JsonObject fullConfig;
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
     * it) and read via {@link #isTopicIncludeRoot()}.
     */
    protected boolean topicIncludeRoot;


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
    }

    ConfigManager(String componentFullName, String componentName, String thingName,
                 ConfigProvider configProvider, JsonObject fullConfig) {
        this(componentFullName, componentName, thingName, configProvider, fullConfig, null);
    }

    ConfigManager(String componentFullName, String componentName, String thingName,
                 ConfigProvider configProvider, JsonObject fullConfig, Platform platform) {
        this.componentFullName = componentFullName;
        this.componentName = componentName;
        this.thingName = thingName;
        this.configProvider = configProvider;
        this.fullConfig = fullConfig;
        this.platform = platform;

        applyConfig(fullConfig);

        // Resolve the component's UNS identity ONCE, from this component's own config
        // (top-level `hierarchy` + `identity`), fail-fast on any inconsistency.
        this.componentIdentity = resolveComponentIdentity();

        // Register logging configuration change listener
        addConfigChangeListener(new LoggingConfigChangeListener(this));
        
        // Note: initializing flag will be set to false by GGCommons after all initialization is complete
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

        tagConfig = ConfigurationFactory.createTagConfiguration(config);
        loggingConfig = ConfigurationFactory.createLoggingConfiguration(config);
        heartbeatConfig = ConfigurationFactory.createHeartbeatConfiguration(config);
        metricConfig = ConfigurationFactory.createMetricConfiguration(config);
        healthConfig = ConfigurationFactory.createHealthConfiguration(config);
        topicIncludeRoot = parseTopicIncludeRoot(config);
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
     * top-level {@code topic.includeRoot} setting, default {@code false}.
     *
     * @return the resolved {@code topic.includeRoot} value
     */
    public boolean isTopicIncludeRoot()
    {
        return topicIncludeRoot;
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
     * {@link com.mbreissi.ggcommons.platform.PlatformResolver#profileMetricLogPath(Platform)}.
     *
     * @return the resolved platform, or {@code null} if not threaded in
     */
    public Platform getPlatform() {
        return platform;
    }

    /**
     * Returns the HTTP health-endpoint configuration (the {@code health} config section, or defaults
     * when absent). Drives the Kubernetes liveness/readiness/startup probe server (FR-HB-1). Whether
     * the server actually starts is decided by {@link com.mbreissi.ggcommons.GGCommons} from the
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

        // 5. component = sanitized short name.
        if (componentName == null || componentName.isEmpty())
        {
            throw identityError("the component short name is not available");
        }
        String componentToken = sanitizedIdentityValue("component", componentName);
        return new MessageIdentity(hier, componentToken, MessageIdentity.DEFAULT_INSTANCE);
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
     * Marks initialization as complete. Called by GGCommons after all initialization is finished.
     * Future configuration changes will trigger listener notifications.
     */
    public void close()
    {
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
     * @param value The raw value to be interpolated (may be null)
     * @return The sanitized value, or an empty string if {@code value} is null
     */
    private static String sanitize(String value)
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
     * Can operate in global mode (controls entire app) or isolated mode (GGCommons only).
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
