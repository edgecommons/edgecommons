/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.platform;

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
 * <p><b>Phase 0:</b> only {@link Platform#GREENGRASS} and {@link Platform#HOST} have profiles, and
 * both default their config source to {@code GG_CONFIG} (a faithful re-expression of today's
 * behavior — HOST does <em>not</em> flip to {@code FILE} until Phase 1). Resolving to
 * {@link Platform#KUBERNETES} fails fast.
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

    /** The library-default identity when no thing name is available (matches today's behavior). */
    public static final String DEFAULT_IDENTITY = "NOT_GREENGRASS";

    /**
     * The platform-profile table (DESIGN-core §3). Phase 0 populates only GREENGRASS and HOST; both
     * deliberately default the config source to {@code GG_CONFIG} to preserve current behavior.
     * KUBERNETES is intentionally absent (declared enum value, no profile yet).
     */
    public static final Map<Platform, PlatformProfile> PROFILES = Map.of(
            Platform.GREENGRASS, new PlatformProfile(Transport.IPC, "GG_CONFIG"),
            Platform.HOST, new PlatformProfile(Transport.MQTT, "GG_CONFIG"));

    private PlatformResolver() {
    }

    /**
     * The parse-time inputs to the resolver. Any field may be {@code null}, meaning "not specified —
     * fall back to detection / the profile default".
     *
     * @param platform   explicit {@code --platform} value, or {@code null} for {@code auto}
     * @param transport  explicit {@code --transport} value, or {@code null} to derive from the platform
     * @param configArgs explicit {@code -c/--config} vector, or {@code null} when {@code -c} is omitted
     * @param thing      explicit {@code -t/--thing} value, or {@code null}
     */
    public record ResolverInputs(Platform platform, Transport transport, String[] configArgs, String thing) {
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
        String basis = autoDetected ? "auto-detected" : "explicit --platform";

        PlatformProfile profile = PROFILES.get(platform);
        if (profile == null) {
            throw new IllegalArgumentException("Platform " + platform + " is not supported in this build "
                    + "(no profile). Valid platforms: GREENGRASS, HOST. (KUBERNETES ships in Phase 1.)");
        }

        Transport transport = inputs.transport() != null ? inputs.transport() : profile.transport();
        validate(platform, transport);

        String[] configSource = inputs.configArgs() != null
                ? inputs.configArgs()
                : new String[]{profile.configSource()};

        String identity = resolveIdentity(inputs.thing(), platform, env);

        LOGGER.info("Resolved platform={} (basis={}) transport={} configSource={} identity={}",
                platform, basis, transport, configSource[0], identity);

        return new ResolvedProfile(platform, transport, configSource, identity);
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
     * Resolves the IoT Thing name / identity (DESIGN-core §6.2). Order: explicit {@code -t/--thing},
     * then the {@code AWS_IOT_THING_NAME} env probe, then the library fallback. For Phase 0 the
     * GREENGRASS and HOST platforms share the same probe, so behavior is unchanged; KUBERNETES
     * Downward-API identity is Phase 1.
     *
     * @param thing    the explicit thing name, or {@code null}
     * @param platform the resolved platform (reserved for the Phase-1 Kubernetes branch)
     * @param env      the process environment
     * @return the resolved identity, never {@code null}
     */
    public static String resolveIdentity(String thing, Platform platform, Map<String, String> env) {
        if (thing != null) {
            return thing;
        }
        String fromEnv = env == null ? null : env.get(ENV_THING_NAME);
        if (fromEnv != null) {
            return fromEnv;
        }
        return DEFAULT_IDENTITY;
    }

    private static boolean isSet(Map<String, String> env, String key) {
        if (env == null) {
            return false;
        }
        String v = env.get(key);
        return v != null && !v.isEmpty();
    }
}
