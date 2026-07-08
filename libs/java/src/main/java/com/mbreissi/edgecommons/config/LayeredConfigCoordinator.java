package com.mbreissi.edgecommons.config;

import com.google.gson.Gson;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParseException;
import com.mbreissi.edgecommons.ParsedCommandLine;
import com.mbreissi.edgecommons.config.provider.ConfigProvider;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.mbreissi.edgecommons.platform.PlatformResolver;
import com.mbreissi.edgecommons.utils.DirectoryWatcher;
import com.mbreissi.edgecommons.utils.FileWatcher;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import software.amazon.awssdk.aws.greengrass.GreengrassCoreIPCClientV2;
import software.amazon.awssdk.aws.greengrass.model.GetConfigurationRequest;
import software.amazon.awssdk.aws.greengrass.model.GetConfigurationResponse;
import software.amazon.awssdk.aws.greengrass.model.GetThingShadowRequest;
import software.amazon.awssdk.aws.greengrass.model.GetThingShadowResponse;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * Owns split-config raw layer state and produces validated effective config snapshots.
 */
final class LayeredConfigCoordinator {
    private static final Logger LOGGER = LogManager.getLogger(LayeredConfigCoordinator.class);
    private static final Gson GSON = new Gson();
    private static final String SHARED_CONFIG_ENV = "EDGECOMMONS_SHARED_CONFIG";
    private static final String SHARED_COMPONENT_ENV = "EDGECOMMONS_SHARED_COMPONENT";
    private static final String DEFAULT_FILE_SHARED = "/etc/edgecommons/shared.json";
    private static final String DEFAULT_SHARED_COMPONENT =
            "com.mbreissi.edgecommons.EdgeCommonsSharedConfig";
    private static final String SHARED_COMPONENT_KEY = "SharedComponentConfig";
    private static final String SHARED_SHADOW = "edgecommons-shared";
    private static final String SHADOW_COMPONENT_CONFIG_KEY = "ComponentConfig";

    private final ConfigProvider componentProvider;
    private final String[] configArgs;
    private final boolean noSharedConfig;
    private final MessagingClient messagingClient;
    private final String thingName;
    private final Map<String, String> env;

    private JsonObject latestComponentLayer;
    private JsonObject latestBaseLayer;
    private BaseResolution latestBaseResolution;
    private ConfigManager configManager;
    private AutoCloseable baseWatch;
    private String baseWatchDescriptor;

    LayeredConfigCoordinator(ConfigProvider componentProvider, ParsedCommandLine cmdLine,
                             MessagingClient messagingClient, String thingName) {
        this(componentProvider, cmdLine, messagingClient, thingName, System.getenv());
    }

    LayeredConfigCoordinator(ConfigProvider componentProvider, ParsedCommandLine cmdLine,
                             MessagingClient messagingClient, String thingName,
                             Map<String, String> env) {
        this.componentProvider = componentProvider;
        this.configArgs = cmdLine.configArgs;
        this.noSharedConfig = cmdLine.noSharedConfig;
        this.messagingClient = messagingClient;
        this.thingName = thingName;
        this.env = env;
    }

    JsonObject loadEffective() {
        EffectiveCandidate candidate = buildCandidate(componentProvider.loadConfiguration());
        accept(candidate);
        return candidate.effective();
    }

    JsonObject reloadEffectiveFromProvider() {
        EffectiveCandidate candidate = buildCandidate(componentProvider.loadConfiguration());
        validate(candidate.effective());
        accept(candidate);
        return candidate.effective();
    }

    JsonObject applyProviderPayload(JsonObject rawPayload) {
        EffectiveCandidate candidate = buildCandidate(rawPayload, true);
        validate(candidate.effective());
        accept(candidate);
        return candidate.effective();
    }

    void attachConfigManager(ConfigManager manager) {
        this.configManager = manager;
        refreshBaseWatch();
    }

    void close() {
        closeBaseWatch();
    }

    private void accept(EffectiveCandidate candidate) {
        latestComponentLayer = candidate.componentLayer();
        latestBaseLayer = candidate.baseLayer();
        latestBaseResolution = candidate.baseResolution();
        refreshBaseWatch();
    }

    private EffectiveCandidate buildCandidate(JsonObject rawPayload) {
        return buildCandidate(rawPayload, false);
    }

    private EffectiveCandidate buildCandidate(JsonObject rawPayload, boolean preserveLegacyBase) {
        LayerPayload payload = parseLayerPayload(rawPayload);
        JsonObject componentLayer = payload.componentLayer();
        validateSharedConfigControl(componentLayer);
        boolean sharedEnabled = sharedEnabled(componentLayer);
        BaseResolution base = BaseResolution.none();
        JsonObject baseLayer = null;
        if (sharedEnabled) {
            if (payload.basePresent()) {
                base = BaseResolution.inline("CONFIG_COMPONENT bundle", payload.baseLayer());
                baseLayer = payload.baseLayer();
            } else if (preserveLegacyBase && isConfigComponent() && latestBaseLayer != null) {
                base = BaseResolution.inline("previous CONFIG_COMPONENT bundle", latestBaseLayer);
                baseLayer = latestBaseLayer;
            } else {
                base = resolveProviderBase(componentLayer);
                baseLayer = base.layer();
            }
        }
        if (baseLayer != null && baseLayer.has(DeepMerge.EXTENDS)) {
            throw new SplitConfigException("N_LAYER_INHERITANCE_NOT_IMPLEMENTED",
                    "Resolved shared config contains 'extends'; N-layer inheritance is not implemented");
        }
        List<JsonObject> layers = new ArrayList<>();
        if (baseLayer != null) {
            layers.add(baseLayer);
        }
        layers.add(componentLayer);
        JsonObject effective = DeepMerge.merge(layers);
        if (baseLayer == null) {
            LOGGER.info(sharedEnabled ? "split-config shared layer absent; using component config only"
                    : "split-config shared layer disabled; using component config only");
        } else {
            LOGGER.info("split-config shared layer applied from {}", base.description());
        }
        return new EffectiveCandidate(componentLayer, baseLayer, base, effective);
    }

    private LayerPayload parseLayerPayload(JsonObject rawPayload) {
        if (rawPayload == null) {
            throw new SplitConfigException("CONFIG_EMPTY", "Configuration source returned no document");
        }
        if (isStructuredError(rawPayload)) {
            JsonObject error = rawPayload.getAsJsonObject("error");
            String code = stringOr(error, "code", "CONFIG_COMPONENT_ERROR");
            String message = stringOr(error, "message", "CONFIG_COMPONENT returned an error");
            throw new SplitConfigException(code, message);
        }
        if (!isConfigComponent()) {
            return new LayerPayload(null, false, requireObject(rawPayload, "component layer"));
        }
        if (!rawPayload.has("base")) {
            return new LayerPayload(null, false,
                    requireObject(rawPayload, "CONFIG_COMPONENT legacy document"));
        }
        JsonElement component = rawPayload.get("component");
        if (component == null || !component.isJsonObject()) {
            throw new SplitConfigException("CONFIG_COMPONENT_BUNDLE_INVALID",
                    "CONFIG_COMPONENT bundle must contain an object 'component' field");
        }
        JsonElement base = rawPayload.get("base");
        if (base != null && !base.isJsonNull() && !base.isJsonObject()) {
            throw new SplitConfigException("CONFIG_COMPONENT_BUNDLE_INVALID",
                    "CONFIG_COMPONENT bundle 'base' must be an object or null");
        }
        return new LayerPayload(base == null || base.isJsonNull() ? null : base.getAsJsonObject(), true,
                component.getAsJsonObject());
    }

    private boolean isStructuredError(JsonObject rawPayload) {
        return rawPayload.has("ok")
                && rawPayload.get("ok").isJsonPrimitive()
                && !rawPayload.get("ok").getAsBoolean()
                && rawPayload.has("error")
                && rawPayload.get("error").isJsonObject();
    }

    private static String stringOr(JsonObject object, String key, String fallback) {
        JsonElement value = object.get(key);
        return value != null && value.isJsonPrimitive() ? value.getAsString() : fallback;
    }

    private boolean sharedEnabled(JsonObject componentLayer) {
        if (noSharedConfig) {
            return false;
        }
        JsonElement sharedConfig = componentLayer.get(DeepMerge.SHARED_CONFIG);
        return sharedConfig == null || sharedConfig.getAsBoolean();
    }

    private void validateSharedConfigControl(JsonObject componentLayer) {
        JsonElement sharedConfig = componentLayer.get(DeepMerge.SHARED_CONFIG);
        if (sharedConfig != null
                && (!sharedConfig.isJsonPrimitive()
                || !sharedConfig.getAsJsonPrimitive().isBoolean())) {
            throw new SplitConfigException("SHARED_CONFIG_INVALID",
                    "sharedConfig must be a boolean when present");
        }
        JsonElement extendsValue = componentLayer.get(DeepMerge.EXTENDS);
        if (extendsValue != null
                && (!extendsValue.isJsonPrimitive()
                || !extendsValue.getAsJsonPrimitive().isString()
                || extendsValue.getAsString().isBlank())) {
            throw new SplitConfigException("SHARED_CONFIG_INVALID",
                    "extends must be a non-empty string when present");
        }
    }

    private BaseResolution resolveProviderBase(JsonObject componentLayer) {
        return switch (providerFamily()) {
            case "FILE" -> resolveFileBase(componentLayer, componentPath(), Paths.get(DEFAULT_FILE_SHARED),
                    false);
            case "CONFIGMAP" -> resolveFileBase(componentLayer, configMapComponentPath(),
                    configMapMountDir().resolve("shared.json"), true);
            case "ENV" -> resolveEnvBase();
            case "GG_CONFIG" -> resolveGreengrassBase();
            case "SHADOW" -> resolveShadowBase();
            default -> BaseResolution.none();
        };
    }

    private BaseResolution resolveFileBase(JsonObject componentLayer, Path componentPath,
                                           Path defaultPath, boolean watchDirectory) {
        JsonElement extendsValue = componentLayer.get(DeepMerge.EXTENDS);
        if (extendsValue != null) {
            Path basePath = Paths.get(extendsValue.getAsString());
            if (!basePath.isAbsolute()) {
                Path parent = componentPath.toAbsolutePath().getParent();
                basePath = (parent == null ? basePath : parent.resolve(basePath)).normalize();
            }
            return readBaseFile(basePath, false, watchDirectory ? configMapMountDir() : null);
        }
        String envPath = env.get(SHARED_CONFIG_ENV);
        if (envPath != null && !envPath.isBlank()) {
            String path = envPath.startsWith("@") ? envPath.substring(1) : envPath;
            return readBaseFile(Paths.get(path), false, watchDirectory ? configMapMountDir() : null);
        }
        return readBaseFile(defaultPath, true, watchDirectory ? configMapMountDir() : null);
    }

    private BaseResolution resolveEnvBase() {
        String raw = env.get(SHARED_CONFIG_ENV);
        if (raw == null || raw.isBlank()) {
            return BaseResolution.none();
        }
        if (raw.startsWith("@")) {
            return readBaseFile(Paths.get(raw.substring(1)), false, null);
        }
        return BaseResolution.inline("ENV " + SHARED_CONFIG_ENV,
                parseObject(raw, "ENV " + SHARED_CONFIG_ENV));
    }

    private BaseResolution resolveGreengrassBase() {
        String component = env.getOrDefault(SHARED_COMPONENT_ENV, DEFAULT_SHARED_COMPONENT);
        boolean explicit = env.containsKey(SHARED_COMPONENT_ENV);
        if (messagingClient == null) {
            if (explicit) {
                throw new SplitConfigException("SHARED_CONFIG_UNAVAILABLE",
                        "MessagingClient required for GG_CONFIG shared config");
            }
            return BaseResolution.none();
        }
        try {
            GreengrassCoreIPCClientV2 ipc =
                    (GreengrassCoreIPCClientV2) messagingClient.getNativeLocalClient();
            GetConfigurationResponse response = ipc.getConfiguration(
                    new GetConfigurationRequest().withComponentName(component));
            JsonObject full = GSON.fromJson(GSON.toJson(response.getValue()), JsonObject.class);
            JsonElement shared = full == null ? null : full.get(SHARED_COMPONENT_KEY);
            if (shared == null || shared.isJsonNull()) {
                if (explicit) {
                    throw new SplitConfigException("SHARED_CONFIG_UNAVAILABLE",
                            "Shared Greengrass config key not found: " + component + "/"
                                    + SHARED_COMPONENT_KEY);
                }
                return BaseResolution.none();
            }
            if (!shared.isJsonObject()) {
                throw new SplitConfigException("SHARED_CONFIG_INVALID",
                        "Shared Greengrass config key must be an object: " + SHARED_COMPONENT_KEY);
            }
            return BaseResolution.inline("GG_CONFIG " + component + "/" + SHARED_COMPONENT_KEY,
                    shared.getAsJsonObject());
        } catch (SplitConfigException e) {
            throw e;
        } catch (Exception e) {
            if (explicit) {
                throw new SplitConfigException("SHARED_CONFIG_UNAVAILABLE",
                        "Shared Greengrass config unavailable: " + e.getMessage(), e);
            }
            LOGGER.info("Default shared Greengrass config unavailable; continuing without a shared layer");
            return BaseResolution.none();
        }
    }

    private BaseResolution resolveShadowBase() {
        if (messagingClient == null) {
            return BaseResolution.none();
        }
        try {
            GreengrassCoreIPCClientV2 ipc =
                    (GreengrassCoreIPCClientV2) messagingClient.getNativeLocalClient();
            GetThingShadowResponse response = ipc.getThingShadow(
                    new GetThingShadowRequest().withThingName(thingName).withShadowName(SHARED_SHADOW));
            byte[] payload = response.getPayload();
            if (payload == null || payload.length == 0) {
                return BaseResolution.none();
            }
            JsonObject shadowDoc = parseObject(new String(payload, StandardCharsets.UTF_8),
                    "shared shadow " + SHARED_SHADOW);
            JsonElement config = shadowComponentConfig(shadowDoc, "desired");
            if (config == null || config.isJsonNull()) {
                config = shadowComponentConfig(shadowDoc, "reported");
            }
            if (config == null || config.isJsonNull()) {
                return BaseResolution.none();
            }
            if (!config.isJsonPrimitive()) {
                throw new SplitConfigException("SHARED_CONFIG_INVALID",
                        "Shared shadow ComponentConfig must be a stringified JSON object");
            }
            JsonObject base = parseObject(config.getAsString(),
                    "shared shadow " + SHADOW_COMPONENT_CONFIG_KEY);
            return BaseResolution.inline("SHADOW " + SHARED_SHADOW + "/" + SHADOW_COMPONENT_CONFIG_KEY,
                    base);
        } catch (SplitConfigException e) {
            throw e;
        } catch (Exception e) {
            LOGGER.info("Shared shadow '{}' unavailable; continuing without a shared layer",
                    SHARED_SHADOW);
            return BaseResolution.none();
        }
    }

    private static JsonElement shadowComponentConfig(JsonObject shadowDoc, String stateLayer) {
        JsonElement state = shadowDoc.get("state");
        if (state == null || !state.isJsonObject()) {
            return null;
        }
        JsonElement layer = state.getAsJsonObject().get(stateLayer);
        if (layer == null || !layer.isJsonObject()) {
            return null;
        }
        return layer.getAsJsonObject().get(SHADOW_COMPONENT_CONFIG_KEY);
    }

    private BaseResolution readBaseFile(Path path, boolean missingIsNoop, Path watchDirectory) {
        if (!Files.exists(path)) {
            if (missingIsNoop) {
                return BaseResolution.missingDefault(path, watchDirectory);
            }
            throw new SplitConfigException("SHARED_CONFIG_UNAVAILABLE",
                    "Shared config file not found: " + path);
        }
        try {
            JsonObject base = parseObject(Files.readString(path, StandardCharsets.UTF_8),
                    "shared config file " + path);
            return BaseResolution.file(path, watchDirectory, base);
        } catch (IOException e) {
            throw new SplitConfigException("SHARED_CONFIG_UNAVAILABLE",
                    "Shared config file unreadable: " + path, e);
        }
    }

    private static JsonObject parseObject(String json, String source) {
        try {
            JsonElement parsed = GSON.fromJson(json, JsonElement.class);
            if (parsed == null || !parsed.isJsonObject()) {
                throw new SplitConfigException("SHARED_CONFIG_INVALID",
                        source + " must be a JSON object");
            }
            return parsed.getAsJsonObject();
        } catch (JsonParseException e) {
            throw new SplitConfigException("SHARED_CONFIG_INVALID",
                    source + " is malformed JSON", e);
        }
    }

    private static JsonObject requireObject(JsonObject object, String name) {
        if (object == null) {
            throw new SplitConfigException("CONFIG_INVALID", name + " must be a JSON object");
        }
        return object;
    }

    private void validate(JsonObject effective) {
        try {
            ConfigurationValidator.validate(effective);
        } catch (ConfigurationValidator.ConfigurationValidationException e) {
            throw new SplitConfigException("CONFIG_VALIDATION_FAILED",
                    "Configuration validation failed: " + e.getMessage(), e);
        }
    }

    private void refreshBaseWatch() {
        if (configManager == null) {
            return;
        }
        BaseResolution resolution = latestBaseResolution;
        String descriptor = resolution == null ? null : resolution.watchDescriptor();
        if (descriptor == null) {
            closeBaseWatch();
            return;
        }
        if (descriptor.equals(baseWatchDescriptor)) {
            return;
        }
        closeBaseWatch();
        baseWatchDescriptor = descriptor;
        Path watchPath = resolution.watchPath();
        FileWatcher.FileChangeHandler handler = () -> {
            LOGGER.info("Shared config changed at {}; reloading effective config", descriptor);
            configManager.reloadFromProvider();
        };
        if (resolution.watchDirectory()) {
            DirectoryWatcher watcher = new DirectoryWatcher(watchPath, handler);
            watcher.setDaemon(true);
            watcher.start();
            baseWatch = watcher::stopThread;
        } else {
            FileWatcher watcher = new FileWatcher(watchPath.toFile(), handler);
            watcher.setDaemon(true);
            watcher.start();
            baseWatch = watcher::stopThread;
        }
    }

    private void closeBaseWatch() {
        if (baseWatch != null) {
            try {
                baseWatch.close();
            } catch (Exception ignored) {
                // stop hooks do not throw today
            }
            baseWatch = null;
        }
        baseWatchDescriptor = null;
    }

    private String providerFamily() {
        return configArgs[0].toUpperCase();
    }

    private boolean isConfigComponent() {
        return "CONFIG_COMPONENT".equals(providerFamily());
    }

    private Path componentPath() {
        return Paths.get(configArgs.length > 1 ? configArgs[1] : "config.json");
    }

    private Path configMapMountDir() {
        return Paths.get(configArgs.length > 1 ? configArgs[1]
                : PlatformResolver.CONFIGMAP_DEFAULT_MOUNT_DIR);
    }

    private Path configMapComponentPath() {
        String key = configArgs.length > 2 ? configArgs[2] : PlatformResolver.CONFIGMAP_DEFAULT_KEY;
        return configMapMountDir().resolve(key);
    }

    private record LayerPayload(JsonObject baseLayer, boolean basePresent, JsonObject componentLayer) {
    }

    private record EffectiveCandidate(JsonObject componentLayer, JsonObject baseLayer,
                                      BaseResolution baseResolution, JsonObject effective) {
    }

    private record BaseResolution(JsonObject layer, String description, Path watchPath,
                                  boolean watchDirectory) {
        static BaseResolution none() {
            return new BaseResolution(null, "none", null, false);
        }

        static BaseResolution inline(String description, JsonObject layer) {
            return new BaseResolution(layer, description, null, false);
        }

        static BaseResolution file(Path path, Path watchDirectory, JsonObject layer) {
            return new BaseResolution(layer, path.toString(),
                    watchDirectory == null ? path : watchDirectory, watchDirectory != null);
        }

        static BaseResolution missingDefault(Path path, Path watchDirectory) {
            return new BaseResolution(null, "missing default " + path, null, false);
        }

        String watchDescriptor() {
            return watchPath == null ? null : (watchDirectory ? "dir:" : "file:") + watchPath;
        }
    }
}
