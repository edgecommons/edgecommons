/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.platform;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;
import java.util.function.Predicate;

/**
 * The pure precedence resolver and platform auto-detector (DESIGN-core §4 / §5). Maps parse-time
 * inputs (explicit flags, then environment, then the platform-profile defaults) to a single
 * {@link ResolvedProfile} consumed by every subsystem initializer.
 *
 * <p>One rule governs every defaultable setting:
 * <pre>
 *   resolve(setting) = explicit flag ▸ platform-profile default ▸ library default
 * </pre>
 *
 * <p>All methods are pure (no I/O beyond the explicitly-injected filesystem probe used for
 * Kubernetes detection), which keeps the resolver and detector unit-testable in isolation.
 *
 * <p><b>Phase 0:</b> {@link Platform#GREENGRASS} and {@link Platform#HOST} both default their config
 * source to {@code GG_CONFIG} (a faithful re-expression of today's behavior — HOST does <em>not</em>
 * flip to {@code FILE} until Phase 1).
 *
 * <p><b>Phase 1a:</b> {@link Platform#KUBERNETES} now has a profile (transport {@code MQTT}, config
 * source {@code CONFIGMAP}) and resolves cleanly — a service-account-token pod auto-detects to it. The
 * IPC&times;KUBERNETES rejection still holds (the IPC lock).
 *
 * <p><b>Phase 1b:</b> two KUBERNETES-platform behaviors land here. (1) FR-MSG-1: under transport
 * {@code MQTT} with the {@code CONFIGMAP} source and no explicit {@code --transport MQTT <path>}, the
 * messaging-config path defaults to the resolved ConfigMap file (see
 * {@link #resolveMessagingConfigPath}), so one mounted {@code config.json} carries both the
 * {@code .messaging} section and the component config. (2) FR-RT-7: {@link #resolveIdentity} gains a
 * KUBERNETES Downward-API tier ({@code GGCOMMONS_THING_NAME}, then {@code POD_NAME}) ahead of the
 * generic {@code AWS_IOT_THING_NAME} probe.
 *
 * <p><b>Phase 1c:</b> the KUBERNETES profile gains a default logging format of
 * {@value #LOGGING_FORMAT_JSON} ({@link PlatformProfile#loggingFormat()}), the stdout-JSON sink
 * (FR-LOG-1), exposed via {@link #profileLoggingFormat(Platform)}; and a default-on HTTP health
 * server ({@link PlatformProfile#healthEnabled()}, FR-HB-1), exposed via
 * {@link #profileHealthEnabled(Platform)}. Both are middle-tier (FR-RT-3) profile defaults consumed
 * downstream (the logging configurator and {@link com.mbreissi.ggcommons.GGCommons} respectively).
 *
 * <p><b>Phase 1d:</b> the KUBERNETES profile gains a default {@code prometheus} metrics target
 * ({@link PlatformProfile#metricTarget()}, FR-MET-1; via {@link #profileMetricTarget(Platform)}) and a
 * default {@code env} credentials key-provider ({@link PlatformProfile#credentialsKeyProvider()},
 * FR-CRED-6; via {@link #profileCredentialsKeyProvider(Platform)}) — the offline-capable software KEK
 * read from a mounted Secret. Both are middle-tier (FR-RT-3) defaults; the credentials default only
 * changes the provider type for an already-configured vault and never auto-enables credentials.
 */
public final class PlatformResolver {

    private static final Logger LOGGER = LogManager.getLogger(PlatformResolver.class);

    /** Nucleus-injected env var pointing at the IPC domain socket (definitive GREENGRASS signal). */
    public static final String ENV_GG_IPC_SOCKET = "AWS_GG_NUCLEUS_DOMAIN_SOCKET_FILEPATH_FOR_COMPONENT";
    /** Nucleus-injected component service-UID (definitive GREENGRASS signal). */
    public static final String ENV_GG_SVCUID = "SVCUID";
    /** Greengrass-injected IoT Thing name (identity probe; mirrors {@code ConfigManagerFactory}). */
    public static final String ENV_THING_NAME = "AWS_IOT_THING_NAME";
    /** Confirming (secondary) Kubernetes signal. The token file is the primary, definitive one. */
    public static final String ENV_K8S_SERVICE_HOST = "KUBERNETES_SERVICE_HOST";
    /** Projected service-account token path: the primary, definitive Kubernetes signal. */
    public static final String K8S_SA_TOKEN_PATH = "/var/run/secrets/kubernetes.io/serviceaccount/token";

    /**
     * KUBERNETES Downward-API identity env var (FR-RT-7): the Helm chart maps the
     * {@code ggcommons.io/thing-name} pod annotation (or an explicit value) into this var. Highest of
     * the KUBERNETES identity tier, ahead of {@link #ENV_K8S_POD_NAME}.
     */
    public static final String ENV_K8S_THING_NAME = "GGCOMMONS_THING_NAME";
    /**
     * KUBERNETES Downward-API pod name env var (FR-RT-7): {@code metadata.name} via a {@code fieldRef}.
     * Used as the identity when {@link #ENV_K8S_THING_NAME} is absent.
     */
    public static final String ENV_K8S_POD_NAME = "POD_NAME";
    /**
     * KUBERNETES Downward-API pod namespace env var ({@code metadata.namespace} via a {@code fieldRef}).
     * A best-effort logging correlation field (FR-LOG-3), wired by the Helm chart in Phase 1b.
     */
    public static final String ENV_K8S_POD_NAMESPACE = "POD_NAMESPACE";
    /**
     * KUBERNETES Downward-API node name env var ({@code spec.nodeName} via a {@code fieldRef}).
     * A best-effort logging correlation field (FR-LOG-3), wired by the Helm chart in Phase 1b.
     */
    public static final String ENV_K8S_NODE_NAME = "NODE_NAME";

    /**
     * The case-insensitive {@code logging.java_format} selector value that selects the structured
     * stdout-JSON logging sink (FR-LOG-1 / FR-LOG-4) — the KUBERNETES profile's default logging
     * format. The same {@code json} token selects the sink in every language (Python
     * {@code python_format}, Rust {@code rust_format}, TS), kept consistent for parity.
     */
    public static final String LOGGING_FORMAT_JSON = "json";

    /**
     * The {@code metricEmission.target} selector value that selects the pull-based prometheus target
     * (FR-MET-1) — the KUBERNETES profile's default metric target. The same {@code prometheus} token
     * selects the target in every language, kept consistent for parity. The target maintains an
     * in-process registry and serves it as OpenMetrics/Prometheus text at an HTTP {@code /metrics}
     * endpoint (pull-based); unlike the push targets (log/messaging/cloudwatch/cloudwatchcomponent) its
     * {@code emit*} only updates the registry and {@code flush()} is a no-op (FR-MET-2).
     */
    public static final String METRIC_TARGET_PROMETHEUS = "prometheus";

    /**
     * The {@code credentials.vault.keyProvider.type} selector value for the env KEY provider
     * (FR-CRED-3 / FR-CRED-6) — the KUBERNETES profile's default key-provider. The env provider sources
     * the vault KEK from a base64-encoded 32-byte key read from an environment variable (typically a
     * mounted k8s Secret), the offline-capable software KEK. The same {@code env} token selects it in
     * every language, kept consistent for parity. It is a <em>middle</em>-tier default: it only changes
     * the default provider type when a {@code credentials} section is already present (it never
     * auto-enables credentials).
     */
    public static final String CREDENTIALS_KEY_PROVIDER_ENV = "env";

    /**
     * The default metric {@code log} file path on platforms WITHOUT the Greengrass logs directory
     * (HOST and KUBERNETES) — the HOST-aware metric-log-path default. Those platforms default
     * {@code metricEmission.targetConfig.logFileName} to this local, writable path (relative to the
     * process working directory; the parent is created on demand) instead of the GREENGRASS
     * {@code /greengrass/v2/logs} default, which does not exist off-device. The effective path follows
     * the precedence — explicit {@code logFileName} config ▸ this platform-profile default ▸ the
     * library default — applied by the metric {@code log} target. Kept consistent across languages
     * (the local {@code ./logs/} directory); the filename suffix matches each library's own default.
     */
    public static final String METRIC_LOG_PATH_LOCAL = "./logs/{ComponentFullName}.metric.log";

    /** The library-default identity when no thing name is available (matches today's behavior). */
    public static final String DEFAULT_IDENTITY = "NOT_GREENGRASS";

    /**
     * The CONFIGMAP config-source token (the k8s-native source / the KUBERNETES profile default).
     * Used to detect the CONFIGMAP source when defaulting the MQTT messaging-config path (FR-MSG-1).
     */
    public static final String CONFIGMAP_SOURCE = "CONFIGMAP";
    /**
     * Default ConfigMap mount directory — the single source of truth shared with
     * {@code ConfigMapConfigProvider} (FR-MSG-1 / FR-CFG-1). A pod with a ConfigMap mounted here loads
     * {@code config.json} with no {@code -c} flag.
     */
    public static final String CONFIGMAP_DEFAULT_MOUNT_DIR = "/etc/ggcommons";
    /** Default ConfigMap key (file name within the mount), shared with {@code ConfigMapConfigProvider}. */
    public static final String CONFIGMAP_DEFAULT_KEY = "config.json";

    /**
     * The platform-profile table (DESIGN-core §3). GREENGRASS defaults the config source to
     * {@code GG_CONFIG} (the Nucleus-managed deployment config); HOST defaults to {@code FILE}
     * (Phase 1, §12 #1 — {@code GG_CONFIG} needs the Nucleus IPC that HOST lacks, so it was a latent
     * footgun). Both carry no logging-format default ({@code null} → the library console/text
     * default). KUBERNETES (Phase 1a) defaults to the {@code MQTT} transport and the k8s-native
     * {@code CONFIGMAP} config source, and (Phase 1c) defaults the logging format to
     * {@value #LOGGING_FORMAT_JSON} — the stdout-JSON sink (FR-LOG-1).
     *
     * <p>Phase 1d models the KUBERNETES profile's metric target ({@value #METRIC_TARGET_PROMETHEUS})
     * and credentials key-provider default ({@value #CREDENTIALS_KEY_PROVIDER_ENV} — the env/software
     * KEK, FR-CRED-6). The streaming (PVC buffer) default is not yet modeled here — it keeps its
     * current library default until its sub-phase ships.
     */
    public static final Map<Platform, PlatformProfile> PROFILES = Map.of(
            Platform.GREENGRASS, new PlatformProfile(Transport.IPC, "GG_CONFIG", null, false, null, null, null),
            Platform.HOST, new PlatformProfile(Transport.MQTT, "FILE", null, false, null, null, METRIC_LOG_PATH_LOCAL),
            Platform.KUBERNETES, new PlatformProfile(Transport.MQTT, "CONFIGMAP", LOGGING_FORMAT_JSON,
                    true, METRIC_TARGET_PROMETHEUS, CREDENTIALS_KEY_PROVIDER_ENV, METRIC_LOG_PATH_LOCAL));

    private PlatformResolver() {
    }

    /**
     * The parse-time inputs to the resolver. Any field may be {@code null}, meaning "not specified —
     * fall back to detection / the profile default".
     *
     * @param platform            explicit {@code --platform} value, or {@code null} for {@code auto}
     * @param transport           explicit {@code --transport} value, or {@code null} to derive from the platform
     * @param configArgs          explicit {@code -c/--config} vector, or {@code null} when {@code -c} is omitted
     * @param thing               explicit {@code -t/--thing} value, or {@code null}
     * @param messagingConfigPath explicit {@code --transport MQTT <path>} payload, or {@code null}
     *                            (the resolver may then synthesize the FR-MSG-1 CONFIGMAP default)
     */
    public record ResolverInputs(Platform platform, Transport transport, String[] configArgs, String thing,
                                 String messagingConfigPath) {
        /**
         * Convenience constructor for callers (and tests) that do not supply an explicit MQTT
         * messaging-config path; equivalent to passing {@code null} for {@code messagingConfigPath}.
         */
        public ResolverInputs(Platform platform, Transport transport, String[] configArgs, String thing) {
            this(platform, transport, configArgs, thing, null);
        }
    }

    /**
     * Resolves the runtime profile from parse-time inputs and the environment (DESIGN-core §4).
     *
     * @param inputs the parsed CLI flags (any field {@code null} = unset)
     * @param env    the process environment (typically {@code System.getenv()})
     * @return the fully resolved profile
     * @throws IllegalArgumentException if the resolved platform has no Phase-0 profile (KUBERNETES),
     *                                  or the platform/transport combination is illegal (IPC lock)
     */
    public static ResolvedProfile resolveProfile(ResolverInputs inputs, Map<String, String> env) {
        boolean autoDetected = inputs.platform() == null;
        Platform platform = autoDetected ? detectPlatform(env) : inputs.platform();

        PlatformProfile profile = PROFILES.get(platform);
        if (profile == null) {
            throw new IllegalArgumentException("Platform " + platform + " is not supported in this build "
                    + "(no profile). Valid platforms: " + PROFILES.keySet() + ".");
        }

        Transport transport = inputs.transport() != null ? inputs.transport() : profile.transport();
        validate(platform, transport);

        String[] configSource = inputs.configArgs() != null
                ? inputs.configArgs()
                : new String[]{profile.configSource()};

        String identity = resolveIdentity(inputs.thing(), platform, env);

        String messagingConfigPath = resolveMessagingConfigPath(
                inputs.messagingConfigPath(), transport, configSource);

        return new ResolvedProfile(platform, transport, configSource, identity, messagingConfigPath);
    }

    /**
     * Resolves the MQTT messaging-config path (FR-MSG-1). The explicit {@code --transport MQTT <path>}
     * payload always wins. Otherwise, <b>only</b> under transport {@code MQTT} <i>and</i> the
     * {@code CONFIGMAP} config source, the path defaults to the resolved ConfigMap file — the same
     * mount dir + key the CONFIGMAP source resolves from ({@code -c CONFIGMAP [dir] [key]} or the
     * profile default {@value #CONFIGMAP_DEFAULT_MOUNT_DIR}/{@value #CONFIGMAP_DEFAULT_KEY}). The
     * single mounted ConfigMap file then doubles as both the messaging config (its {@code .messaging}
     * section) and the component config.
     *
     * <p>Computed from parse-time inputs only (the resolved transport + config source), <em>before</em>
     * messaging init — the ConfigMap is never read via the config source first. HOST is unaffected (it
     * defaults to {@code GG_CONFIG}, not {@code CONFIGMAP}, so HOST+MQTT still requires an explicit
     * path).
     *
     * @param explicit     the explicit {@code --transport MQTT <path>} payload, or {@code null}
     * @param transport    the resolved transport
     * @param configSource the resolved config-source vector ({@code [SOURCE, args...]})
     * @return the explicit path if present; else the CONFIGMAP default under MQTT+CONFIGMAP; else {@code null}
     */
    static String resolveMessagingConfigPath(String explicit, Transport transport, String[] configSource) {
        if (explicit != null) {
            return explicit;  // explicit path always wins (behavior unchanged)
        }
        if (transport == Transport.MQTT && configSource != null && configSource.length > 0
                && CONFIGMAP_SOURCE.equalsIgnoreCase(configSource[0])) {
            String mountDir = configSource.length > 1 ? configSource[1] : CONFIGMAP_DEFAULT_MOUNT_DIR;
            String key = configSource.length > 2 ? configSource[2] : CONFIGMAP_DEFAULT_KEY;
            // Resolve exactly as ConfigMapConfigProvider does (mountDir.resolve(key)) so this is
            // literally the same file path the CONFIGMAP source will load the component config from.
            return Path.of(mountDir).resolve(key).toString();
        }
        return null;
    }

    /**
     * Auto-detects the platform from the environment (DESIGN-core §5), using the default filesystem
     * probe for the Kubernetes service-account token. First match wins; HOST is the fallback.
     *
     * @param env the process environment
     * @return the detected platform
     */
    public static Platform detectPlatform(Map<String, String> env) {
        return detectPlatform(env, p -> Files.exists(Path.of(p)));
    }

    /**
     * Auto-detection with an injectable filesystem probe (for tests). Signal order is load-bearing:
     * a containerized Nucleus component can set both Greengrass and Kubernetes signals, and
     * GREENGRASS must win (DESIGN-core §5).
     *
     * @param env        the process environment
     * @param fileExists predicate answering whether a given path exists (e.g. the SA token)
     * @return the detected platform
     */
    static Platform detectPlatform(Map<String, String> env, Predicate<String> fileExists) {
        // 1. GREENGRASS — Nucleus-injected signals exist nowhere else (definitive).
        if (isSet(env, ENV_GG_IPC_SOCKET) || isSet(env, ENV_GG_SVCUID)) {
            return Platform.GREENGRASS;
        }
        // 2. KUBERNETES — projected SA token (primary); service host (confirming/secondary).
        if (fileExists.test(K8S_SA_TOKEN_PATH) || isSet(env, ENV_K8S_SERVICE_HOST)) {
            return Platform.KUBERNETES;
        }
        // 3. HOST — fallback.
        return Platform.HOST;
    }

    /**
     * Validates the platform/transport combination — the IPC lock (DESIGN-core §4.1). IPC is valid
     * only on a Greengrass Nucleus, which provides the IPC domain socket.
     *
     * @param platform  the resolved platform
     * @param transport the resolved transport
     * @throws IllegalArgumentException if {@code transport == IPC && platform != GREENGRASS}
     */
    public static void validate(Platform platform, Transport transport) {
        if (transport == Transport.IPC && platform != Platform.GREENGRASS) {
            throw new IllegalArgumentException("IPC transport requires --platform GREENGRASS (the "
                    + "Nucleus provides the IPC socket); got platform=" + platform);
        }
    }

    /**
     * Returns the platform-profile's default {@code logging.<lang>_format} token (FR-LOG-1/4,
     * precedence FR-RT-3) — {@value #LOGGING_FORMAT_JSON} on {@link Platform#KUBERNETES}, {@code null}
     * on GREENGRASS/HOST (no override → the library console/text default). This is the <em>middle</em>
     * precedence tier consumed by the logging configurator
     * ({@link com.mbreissi.ggcommons.config.ConfigManager#reconfigureLogging}): the resolved
     * platform is known before the component config loads, so the profile default can be applied when
     * the config omits an explicit {@code logging.java_format}. Mirrors Rust
     * {@code platform::profile(p).logging_format} and Python {@code profile_logging_format(p)}.
     *
     * @param platform the resolved platform, or {@code null}
     * @return the profile's default logging-format token, or {@code null} when none applies
     */
    public static String profileLoggingFormat(Platform platform) {
        if (platform == null) {
            return null;
        }
        PlatformProfile profile = PROFILES.get(platform);
        return profile == null ? null : profile.loggingFormat();
    }

    /**
     * Returns the platform-profile's default for the HTTP health server (FR-HB-1, precedence FR-RT-3)
     * — {@code true} on {@link Platform#KUBERNETES} (the liveness/readiness endpoint starts by default,
     * no config needed), {@code false} on GREENGRASS / HOST (opt-in via {@code health.enabled=true})
     * and for a {@code null}/unknown platform. This is the <em>middle</em> precedence tier consumed by
     * {@link com.mbreissi.ggcommons.GGCommons}: an explicit {@code health.enabled} from the config
     * wins; otherwise this profile default decides. Mirrors {@link #profileLoggingFormat(Platform)} and
     * the Rust {@code platform::profile(p).health_enabled} / Python {@code profile_health_enabled(p)}.
     *
     * @param platform the resolved platform, or {@code null}
     * @return {@code true} if the health server should default to on for this platform
     */
    public static boolean profileHealthEnabled(Platform platform) {
        if (platform == null) {
            return false;
        }
        PlatformProfile profile = PROFILES.get(platform);
        return profile != null && profile.healthEnabled();
    }

    /**
     * Returns the platform-profile's default {@code metricEmission.target} token (FR-MET-1/4,
     * precedence FR-RT-3) — {@value #METRIC_TARGET_PROMETHEUS} on {@link Platform#KUBERNETES} (the
     * pull-based in-process registry + {@code /metrics} HTTP endpoint), {@code null} on GREENGRASS/HOST
     * and for a {@code null}/unknown platform (no override; the library default {@code "log"} applies).
     * This is the <em>middle</em> precedence tier consumed by
     * {@link com.mbreissi.ggcommons.metrics.MetricEmitter}: an explicit {@code metricEmission.target}
     * from the config wins; otherwise this profile default decides; else the library default. Mirrors
     * {@link #profileLoggingFormat(Platform)} / {@link #profileHealthEnabled(Platform)} and the Rust
     * {@code platform::profile(p).metric_target} / Python {@code profile_metric_target(p)} / TS
     * {@code profileMetricTarget(p)}.
     *
     * <p><b>Rust feature nuance (parity note):</b> in the Rust port the KUBERNETES default resolves to
     * {@code prometheus} only when the {@code metrics-prometheus} cargo feature is compiled in, and
     * gracefully falls back to {@code log} (with a warning) otherwise. Java has no compile-time feature
     * gate — the prometheus client is always on the classpath of the shaded JAR — so the Java default is
     * unconditionally {@value #METRIC_TARGET_PROMETHEUS} on KUBERNETES.
     *
     * @param platform the resolved platform, or {@code null}
     * @return the profile's default metric-target token, or {@code null} when none applies
     */
    public static String profileMetricTarget(Platform platform) {
        if (platform == null) {
            return null;
        }
        PlatformProfile profile = PROFILES.get(platform);
        return profile == null ? null : profile.metricTarget();
    }

    /**
     * Returns the platform-profile's default metric {@code log} file path, or {@code null} (the
     * HOST-aware metric-log-path default). {@link #METRIC_LOG_PATH_LOCAL} (a local, writable path) on
     * {@link Platform#HOST} / {@link Platform#KUBERNETES} — neither has the GREENGRASS
     * {@code /greengrass/v2/logs} directory — {@code null} on GREENGRASS and for a {@code null}/unknown
     * platform (no override; the library default {@code /greengrass/v2/logs} applies). This is the
     * <em>middle</em> precedence tier consumed by the metric {@code log} target: an explicit
     * {@code metricEmission.targetConfig.logFileName} wins; otherwise this profile default decides;
     * else the library default. Mirrors {@link #profileMetricTarget(Platform)} and the Rust
     * {@code profile(p).metric_log_path} / Python {@code profile_metric_log_path(p)} / TS
     * {@code profileMetricLogPath(p)}.
     *
     * @param platform the resolved platform, or {@code null}
     * @return the profile's default metric-log path template, or {@code null} when none applies
     */
    public static String profileMetricLogPath(Platform platform) {
        if (platform == null) {
            return null;
        }
        PlatformProfile profile = PROFILES.get(platform);
        return profile == null ? null : profile.metricLogPath();
    }

    /**
     * Returns the platform-profile's default {@code credentials.vault.keyProvider.type} token
     * (FR-CRED-3 / FR-CRED-6, precedence FR-RT-3) — {@value #CREDENTIALS_KEY_PROVIDER_ENV} on
     * {@link Platform#KUBERNETES} (the env/software KEK), {@code null} on GREENGRASS/HOST and for a
     * {@code null}/unknown platform (no override; the library default {@code "file"} applies). This is
     * the <em>middle</em> precedence tier consumed by
     * {@link com.mbreissi.ggcommons.credentials.Credentials#open(com.google.gson.JsonObject, String, String)}
     * at the credentials init site: an explicit {@code keyProvider.type} from the config wins;
     * otherwise this profile default decides; else the library default {@code "file"}. Mirrors
     * {@link #profileLoggingFormat(Platform)} / {@link #profileHealthEnabled(Platform)} /
     * {@link #profileMetricTarget(Platform)} and the Rust
     * {@code platform::profile(p).credentials_key_provider} / Python
     * {@code profile_credentials_key_provider(p)} / TS {@code profileCredentialsKeyProvider(p)}.
     *
     * <p><b>Important:</b> this only changes the <em>default provider type</em> for a credentials
     * vault that is <em>already configured</em> (a {@code credentials} section is present). It never
     * auto-enables credentials — a component with no {@code credentials} section keeps no vault on any
     * platform.
     *
     * @param platform the resolved platform, or {@code null}
     * @return the profile's default credentials key-provider token, or {@code null} when none applies
     */
    public static String profileCredentialsKeyProvider(Platform platform) {
        if (platform == null) {
            return null;
        }
        PlatformProfile profile = PROFILES.get(platform);
        return profile == null ? null : profile.credentialsKeyProvider();
    }

    /**
     * Resolves the IoT Thing name / identity (DESIGN-core §6.2, FR-RT-7 / FR-CFG-6). Order:
     * <ol>
     *   <li>explicit {@code -t/--thing} (highest);</li>
     *   <li><b>only when {@code platform == KUBERNETES}</b> the Downward-API env tier, in order:
     *       {@link #ENV_K8S_THING_NAME} ({@code GGCOMMONS_THING_NAME}) then {@link #ENV_K8S_POD_NAME}
     *       ({@code POD_NAME});</li>
     *   <li>the generic {@code AWS_IOT_THING_NAME} probe (GREENGRASS / platform-supplied);</li>
     *   <li>the library fallback {@link #DEFAULT_IDENTITY}.</li>
     * </ol>
     *
     * <p>The KUBERNETES tier (2) takes precedence over the generic probe (3) <b>only</b> on the
     * KUBERNETES platform; on every other platform behavior is unchanged (the {@code platform}
     * argument is now load-bearing). Empty env values are treated as absent at every tier. The
     * resolved value is not mangled here — it is sanitized later by template substitution
     * ({@link com.mbreissi.ggcommons.config.ConfigManager#resolveTemplate}) wherever it is
     * interpolated into a path/topic.
     *
     * @param thing    the explicit thing name, or {@code null}
     * @param platform the resolved platform (selects the KUBERNETES Downward-API tier)
     * @param env      the process environment
     * @return the resolved identity, never {@code null}
     */
    public static String resolveIdentity(String thing, Platform platform, Map<String, String> env) {
        if (thing != null) {
            return thing;
        }
        // KUBERNETES Downward-API identity tier — precedes the generic probe only on k8s.
        if (platform == Platform.KUBERNETES) {
            String fromAnnotation = nonEmpty(env, ENV_K8S_THING_NAME);
            if (fromAnnotation != null) {
                return fromAnnotation;
            }
            String fromPod = nonEmpty(env, ENV_K8S_POD_NAME);
            if (fromPod != null) {
                return fromPod;
            }
        }
        String fromEnv = nonEmpty(env, ENV_THING_NAME);  // empty AWS_IOT_THING_NAME treated as absent
        if (fromEnv != null) {
            return fromEnv;
        }
        return DEFAULT_IDENTITY;
    }

    /**
     * Returns the env value for {@code key} if present and non-empty, else {@code null}.
     */
    private static String nonEmpty(Map<String, String> env, String key) {
        if (env == null) {
            return null;
        }
        String v = env.get(key);
        return (v != null && !v.isEmpty()) ? v : null;
    }

    private static boolean isSet(Map<String, String> env, String key) {
        if (env == null) {
            return false;
        }
        String v = env.get(key);
        return v != null && !v.isEmpty();
    }
}
