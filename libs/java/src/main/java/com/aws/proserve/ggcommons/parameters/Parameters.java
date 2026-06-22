package com.aws.proserve.ggcommons.parameters;

import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import com.aws.proserve.ggcommons.credentials.Credentials;
import com.aws.proserve.ggcommons.credentials.KeyProvider;
import com.aws.proserve.ggcommons.credentials.LocalVault;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;

/**
 * Factory: build a {@link DefaultParameterService} from the {@code parameters} config section.
 *
 * <p>Phase 1 ships three sources: {@code env}, {@code mountedDir}, and {@code awsSsm} (optional —
 * needs the {@code software.amazon.awssdk:ssm} dependency on the classpath). The cache decision is
 * <b>source-aware</b> (a remote source persists encrypted so values survive restarts/offline; a
 * local source uses an in-memory cache), overridable via {@code cache.persist}. Numeric fields parse
 * leniently because Greengrass delivers config numbers as doubles. Config is parsed from a Gson
 * {@link JsonObject} (no POJOs), matching the credentials style. Mirrors the Rust
 * {@code parameters::config::open}.
 *
 * <p><b>No key namespacing:</b> parameter keys are <i>not</i> namespaced (matching the Rust port);
 * isolation comes from the per-component templated {@code cache.path} instead.
 */
public final class Parameters {
    private static final Logger LOGGER = LogManager.getLogger(Parameters.class);

    private Parameters() {}

    public static DefaultParameterService open(JsonObject parametersConfig) {
        return open(parametersConfig, "");
    }

    /**
     * Build a {@link DefaultParameterService} from a parsed {@code parameters} config object.
     *
     * @param parametersConfig the {@code parameters} config section (may be {@code null} → defaults)
     * @param namespace        present for signature parity with {@link Credentials#open}; parameter
     *                         keys are intentionally <b>not</b> namespaced (per the Rust port)
     * @return the built service (with bootstrap + background refresh applied)
     */
    public static DefaultParameterService open(JsonObject parametersConfig, String namespace) {
        JsonObject cfg = parametersConfig != null ? parametersConfig : new JsonObject();

        JsonObject sourceCfg = cfg.has("source") ? cfg.getAsJsonObject("source") : new JsonObject();
        String kind = sourceCfg.has("type") ? sourceCfg.get("type").getAsString() : "none";

        long refreshIntervalSecs = cfg.has("refreshIntervalSecs")
                ? (long) cfg.get("refreshIntervalSecs").getAsDouble() // lenient: Greengrass sends doubles (300.0)
                : 300;
        boolean bootstrapOnStart = !cfg.has("bootstrapOnStart") || cfg.get("bootstrapOnStart").getAsBoolean();

        JsonObject sync = cfg.has("sync") ? cfg.getAsJsonObject("sync") : new JsonObject();
        List<String> syncNames = parseNames(sync);
        List<Map.Entry<String, Boolean>> syncPaths = parsePaths(sync);

        ParameterSource source = buildSource(kind, sourceCfg);

        // Source-aware default: remote sources persist encrypted (survive restart/offline); local
        // sources stay in memory (the backend is itself always available). `cache.persist` overrides.
        JsonObject cacheCfg = cfg.has("cache") ? cfg.getAsJsonObject("cache") : new JsonObject();
        boolean persist = cacheCfg.has("persist")
                ? cacheCfg.get("persist").getAsBoolean()
                : isRemote(kind);

        DefaultParameterService service;
        if (persist) {
            String path = cacheCfg.has("path") ? cacheCfg.get("path").getAsString() : "param-cache";
            JsonObject kp = cacheCfg.has("keyProvider") ? cacheCfg.getAsJsonObject("keyProvider") : new JsonObject();
            KeyProvider provider = Credentials.buildKeyProvider(kp, path + ".key");
            // keepVersions = 1: the cache only ever needs the latest value of each parameter.
            LocalVault vault = LocalVault.open(Paths.get(path), provider, 1);
            service = DefaultParameterService.withPersistentCache(source, vault, new Object(), syncNames, syncPaths);
        } else {
            service = DefaultParameterService.withMemoryCache(source, syncNames, syncPaths);
        }

        if (bootstrapOnStart) {
            // Offline-first: a bootstrap failure is non-fatal — the component starts and can retry via
            // refresh(). A persisted cache from a prior run still serves reads while the source is down.
            try {
                service.refresh();
            } catch (RuntimeException e) {
                LOGGER.warn("parameter bootstrap refresh failed (continuing; cache may be empty): {}", e.getMessage());
            }
        }

        // Background refresh on the configured interval (0 disables; close() stops the thread).
        return service.withRefresh(refreshIntervalSecs);
    }

    /** Whether {@code kind} is a remote (network-backed) source — drives the default cache persistence. */
    private static boolean isRemote(String kind) {
        return "awsSsm".equals(kind);
    }

    /** Build the {@link ParameterSource} backend named by {@code source.type}. */
    private static ParameterSource buildSource(String kind, JsonObject sourceCfg) {
        switch (kind) {
            case "env": {
                String prefix = sourceCfg.has("prefix") ? sourceCfg.get("prefix").getAsString() : "GG_PARAM_";
                return new EnvSource(prefix);
            }
            case "mountedDir": {
                if (!sourceCfg.has("root")) {
                    throw new ParameterException("mountedDir source requires source.root");
                }
                return new MountedDirSource(sourceCfg.get("root").getAsString(), stringList(sourceCfg, "securePaths"));
            }
            case "awsSsm": {
                // The AwsSsmSource class (and its software.amazon.awssdk.services.ssm imports) is only
                // referenced here, so the optional `ssm` dependency being absent never breaks the
                // env/mountedDir sources. Mirrors how the credentials central source is gated.
                String region = sourceCfg.has("region") ? sourceCfg.get("region").getAsString() : null;
                String endpoint = sourceCfg.has("endpointUrl") ? sourceCfg.get("endpointUrl").getAsString() : null;
                boolean withDecryption = !sourceCfg.has("withDecryption")
                        || sourceCfg.get("withDecryption").getAsBoolean();
                try {
                    return new AwsSsmSource(region, endpoint, withDecryption);
                } catch (NoClassDefFoundError e) {
                    throw new ParameterException("awsSsm source requires the optional 'software.amazon.awssdk:ssm' "
                            + "dependency on the classpath", e);
                }
            }
            default:
                throw new ParameterException("parameter source '" + kind + "' is not supported "
                        + "(supported: 'env', 'mountedDir', 'awsSsm')");
        }
    }

    /** Parse {@code sync.names} — an array of bare strings. */
    private static List<String> parseNames(JsonObject sync) {
        return stringList(sync, "names");
    }

    /** Parse {@code sync.paths} — each entry a bare string (recursive) or {@code {path, recursive}}. */
    private static List<Map.Entry<String, Boolean>> parsePaths(JsonObject sync) {
        List<Map.Entry<String, Boolean>> out = new ArrayList<>();
        if (!sync.has("paths")) {
            return out;
        }
        for (JsonElement el : sync.getAsJsonArray("paths")) {
            if (el.isJsonPrimitive()) {
                out.add(Map.entry(el.getAsString(), true)); // bare string => recursive
            } else if (el.isJsonObject()) {
                JsonObject o = el.getAsJsonObject();
                String path = o.get("path").getAsString();
                boolean recursive = !o.has("recursive") || o.get("recursive").getAsBoolean();
                out.add(Map.entry(path, recursive));
            }
        }
        return out;
    }

    private static List<String> stringList(JsonObject obj, String key) {
        List<String> out = new ArrayList<>();
        if (obj.has(key) && obj.get(key).isJsonArray()) {
            for (JsonElement el : obj.getAsJsonArray(key)) {
                out.add(el.getAsString());
            }
        }
        return out;
    }
}
