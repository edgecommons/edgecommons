package com.mbreissi.edgecommons.config;

import com.google.gson.Gson;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonPrimitive;
import com.mbreissi.edgecommons.ParsedCommandLine;
import com.mbreissi.edgecommons.config.provider.ConfigComponentProvider;
import com.mbreissi.edgecommons.config.provider.ConfigProvider;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Produces the single effective config snapshot from the active provider.
 *
 * <p>Direct providers are single-document sources. CONFIG_COMPONENT is the only provider that
 * carries hierarchy on the wire, and it must send lineage bundles containing ordered
 * {@code layers[].config} fragments.
 */
final class LayeredConfigCoordinator {
    private static final Logger LOGGER = LogManager.getLogger(LayeredConfigCoordinator.class);
    private static final Gson GSON = new Gson();

    private final ConfigProvider componentProvider;
    private final String[] configArgs;
    private final String requestedComponent;

    private JsonObject latestEffective;

    LayeredConfigCoordinator(ConfigProvider componentProvider, ParsedCommandLine cmdLine,
                             MessagingClient messagingClient, String thingName) {
        this.componentProvider = componentProvider;
        this.configArgs = cmdLine.configArgs;
        this.requestedComponent = componentProvider instanceof ConfigComponentProvider provider
                ? provider.getComponentToken()
                : null;
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
        EffectiveCandidate candidate = buildCandidate(rawPayload);
        validate(candidate.effective());
        accept(candidate);
        return candidate.effective();
    }

    void attachConfigManager(ConfigManager manager) {
        // No-op: hierarchy is resolved by ConfigComponent, not by local shared/base watchers.
    }

    void close() {
        // No resources are owned by the coordinator after the hierarchical replacement.
    }

    private void accept(EffectiveCandidate candidate) {
        latestEffective = candidate.effective();
    }

    private EffectiveCandidate buildCandidate(JsonObject rawPayload) {
        if (rawPayload == null) {
            throw new HierarchicalConfigException("CONFIG_EMPTY", "Configuration source returned no document");
        }
        if (!isConfigComponent()) {
            return new EffectiveCandidate(rawPayload.deepCopy());
        }
        LineagePayload payload = parseLineagePayload(rawPayload);
        JsonObject effective = DeepMerge.merge(payload.layers());
        LOGGER.info("hierarchical config lineage bundle applied: component={}, catalogVersion={}, layers={}",
                payload.component(), payload.catalogVersion(), payload.layers().size());
        return new EffectiveCandidate(effective);
    }

    private LineagePayload parseLineagePayload(JsonObject rawPayload) {
        if (isStructuredError(rawPayload)) {
            JsonObject error = rawPayload.getAsJsonObject("error");
            String code = stringOr(error, "code", "CONFIG_COMPONENT_ERROR");
            String message = stringOr(error, "message", "CONFIG_COMPONENT returned an error");
            throw new HierarchicalConfigException(code, message);
        }

        requireLineageVersion(rawPayload);
        String catalogVersion = requireString(rawPayload, "catalogVersion");
        String component = requireString(rawPayload, "component");
        if (requestedComponent != null && !requestedComponent.equals(component)) {
            throw lineageInvalid("CONFIG_COMPONENT lineage bundle component '" + component
                    + "' does not match requested component '" + requestedComponent + "'");
        }
        JsonArray layers = requireNonEmptyArray(rawPayload, "layers");

        List<JsonObject> configs = new ArrayList<>();
        Map<String, String> scopeOwners = new LinkedHashMap<>();
        Map<String, JsonElement> identityOwners = new LinkedHashMap<>();
        for (int index = 0; index < layers.size(); index++) {
            JsonElement layerElement = layers.get(index);
            if (layerElement == null || !layerElement.isJsonObject()) {
                throw lineageInvalid("CONFIG_COMPONENT lineage bundle layers must be objects");
            }
            JsonObject layer = layerElement.getAsJsonObject();
            String layerId = requireString(layer, "id");
            String kind = requireString(layer, "kind");
            if (!"scope".equals(kind) && !"component".equals(kind)) {
                throw lineageInvalid("CONFIG_COMPONENT lineage bundle layer '" + layerId
                        + "' kind must be 'scope' or 'component'");
            }
            if ("component".equals(kind)) {
                if (index != layers.size() - 1) {
                    throw lineageInvalid("CONFIG_COMPONENT lineage bundle component layer must be final");
                }
                String layerComponent = requireString(layer, "component");
                if (!component.equals(layerComponent)) {
                    throw lineageInvalid("CONFIG_COMPONENT lineage bundle component layer '" + layerId
                            + "' does not match bundle component '" + component + "'");
                }
            } else if (index == layers.size() - 1) {
                throw lineageInvalid("CONFIG_COMPONENT lineage bundle final layer must be kind 'component'");
            } else if (!layer.has("scope") || !layer.get("scope").isJsonObject()) {
                throw lineageInvalid("CONFIG_COMPONENT lineage bundle scope layer '" + layerId
                        + "' must contain object scope");
            }
            validateScopeOwnership(layer, scopeOwners);
            JsonObject config = requireObjectField(layer, "config");
            validateIdentityOwnership(config, identityOwners);
            configs.add(config);
        }
        return new LineagePayload(catalogVersion, component, configs);
    }

    private static void requireLineageVersion(JsonObject rawPayload) {
        JsonElement version = rawPayload.get("lineageVersion");
        if (version == null || !version.isJsonPrimitive()) {
            throw lineageInvalid("CONFIG_COMPONENT lineage bundle must contain lineageVersion: 1");
        }
        JsonPrimitive primitive = version.getAsJsonPrimitive();
        if (!primitive.isNumber() || primitive.getAsInt() != 1) {
            throw lineageInvalid("CONFIG_COMPONENT lineage bundle lineageVersion must be 1");
        }
    }

    private static JsonArray requireNonEmptyArray(JsonObject object, String key) {
        JsonElement value = object.get(key);
        if (value == null || !value.isJsonArray() || value.getAsJsonArray().isEmpty()) {
            throw lineageInvalid("CONFIG_COMPONENT lineage bundle must contain a non-empty '"
                    + key + "' array");
        }
        return value.getAsJsonArray();
    }

    private static String requireString(JsonObject object, String key) {
        JsonElement value = object.get(key);
        if (value == null || !value.isJsonPrimitive()
                || !value.getAsJsonPrimitive().isString()
                || value.getAsString().isBlank()) {
            throw lineageInvalid("CONFIG_COMPONENT lineage bundle must contain a non-empty '"
                    + key + "' string");
        }
        return value.getAsString();
    }

    private static JsonObject requireObjectField(JsonObject object, String key) {
        JsonElement value = object.get(key);
        if (value == null || !value.isJsonObject()) {
            throw lineageInvalid("CONFIG_COMPONENT lineage bundle layer '" + key
                    + "' must be an object");
        }
        return value.getAsJsonObject();
    }

    private static void validateScopeOwnership(JsonObject layer, Map<String, String> scopeOwners) {
        JsonElement scopeElement = layer.get("scope");
        if (scopeElement == null || scopeElement.isJsonNull()) {
            return;
        }
        if (!scopeElement.isJsonObject()) {
            throw lineageInvalid("CONFIG_COMPONENT lineage bundle layer scope must be an object");
        }
        JsonObject scope = scopeElement.getAsJsonObject();
        for (String key : scope.keySet()) {
            JsonElement value = scope.get(key);
            if (value == null || !value.isJsonPrimitive()
                    || !value.getAsJsonPrimitive().isString()) {
                throw lineageInvalid("CONFIG_COMPONENT lineage bundle scope values must be strings");
            }
            String newValue = value.getAsString();
            String owned = scopeOwners.get(key);
            if (owned != null && !owned.equals(newValue)) {
                throw new HierarchicalConfigException("LINEAGE_SCOPE_CONFLICT",
                        "Lineage scope key '" + key + "' changed from '" + owned
                                + "' to '" + newValue + "'");
            }
            scopeOwners.putIfAbsent(key, newValue);
        }
    }

    private static void validateIdentityOwnership(JsonObject config,
                                                  Map<String, JsonElement> identityOwners) {
        JsonElement identityElement = config.get("identity");
        if (identityElement == null || identityElement.isJsonNull()) {
            return;
        }
        if (!identityElement.isJsonObject()) {
            return;
        }
        JsonObject identity = identityElement.getAsJsonObject();
        for (String key : identity.keySet()) {
            JsonElement newValue = identity.get(key);
            JsonElement owned = identityOwners.get(key);
            if (owned != null && !owned.equals(newValue)) {
                throw new HierarchicalConfigException("LINEAGE_IDENTITY_CONFLICT",
                        "Lineage identity key '" + key + "' changed from "
                                + GSON.toJson(owned) + " to " + GSON.toJson(newValue));
            }
            identityOwners.putIfAbsent(key, newValue == null ? null : newValue.deepCopy());
        }
    }

    private boolean isStructuredError(JsonObject rawPayload) {
        return rawPayload.has("ok")
                && rawPayload.get("ok").isJsonPrimitive()
                && rawPayload.get("ok").getAsJsonPrimitive().isBoolean()
                && !rawPayload.get("ok").getAsBoolean()
                && rawPayload.has("error")
                && rawPayload.get("error").isJsonObject();
    }

    private static String stringOr(JsonObject object, String key, String fallback) {
        JsonElement value = object.get(key);
        return value != null && value.isJsonPrimitive() ? value.getAsString() : fallback;
    }

    private static HierarchicalConfigException lineageInvalid(String message) {
        return new HierarchicalConfigException("LINEAGE_BUNDLE_INVALID", message);
    }

    private void validate(JsonObject effective) {
        try {
            ConfigurationValidator.validate(effective);
        } catch (ConfigurationValidator.ConfigurationValidationException e) {
            throw new HierarchicalConfigException("CONFIG_VALIDATION_FAILED",
                    "Configuration validation failed: " + e.getMessage(), e);
        }
    }

    private String providerFamily() {
        return configArgs[0].toUpperCase();
    }

    private boolean isConfigComponent() {
        return "CONFIG_COMPONENT".equals(providerFamily());
    }

    private record LineagePayload(String catalogVersion, String component,
                                  List<JsonObject> layers) {
    }

    private record EffectiveCandidate(JsonObject effective) {
    }
}
