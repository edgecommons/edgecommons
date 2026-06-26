/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.config;

import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

/**
 * Configuration for the HTTP health endpoint (FR-HB-1) — the Kubernetes liveness/readiness/startup
 * probe server. Parsed from the {@code health} config section (canonical schema):
 *
 * <pre>
 * "health": {
 *   "enabled": true,                 // optional; see the enablement precedence below
 *   "port": 8081,                    // default {@value #DEFAULT_PORT}
 *   "livenessPath": "/livez",        // default {@value #DEFAULT_LIVENESS_PATH}
 *   "readinessPath": "/readyz",      // default {@value #DEFAULT_READINESS_PATH}
 *   "startupPath": "/startupz"       // default {@value #DEFAULT_STARTUP_PATH}
 * }
 * </pre>
 *
 * <p><b>Enablement precedence (FR-HB-1 / FR-RT-3).</b> Whether the server starts is decided in
 * {@link com.mbreissi.ggcommons.GGCommons}, not here: an explicit {@code health.enabled} (when
 * present) wins ▸ else {@code true} on the {@code KUBERNETES} platform ▸ else {@code false}. This
 * class therefore distinguishes "{@code enabled} was set" ({@link #isEnabledExplicitlySet()}) from
 * its value ({@link #isEnabled()}), mirroring how {@link LoggingConfiguration#isFormatExplicitlySet()}
 * drives the logging-format precedence.
 *
 * <p>Mirrors the Python/Rust/TS health config for four-way parity (Java is canonical).
 */
public class HealthConfiguration
{
    protected static final Logger LOGGER = LogManager.getLogger(HealthConfiguration.class);

    /** Default TCP port the health server binds (matches the canonical schema default). */
    public static final int DEFAULT_PORT = 8081;
    /** Default liveness probe path (200 while the process is alive; never checks the broker). */
    public static final String DEFAULT_LIVENESS_PATH = "/livez";
    /** Default readiness probe path (200 only when connected and ready; 503 on startup/shutdown). */
    public static final String DEFAULT_READINESS_PATH = "/readyz";
    /** Default startup probe path (reuses readiness semantics; for slow connects). */
    public static final String DEFAULT_STARTUP_PATH = "/startupz";

    // null => the `enabled` key was absent (defer to the platform-profile default); non-null => an
    // explicit config value that overrides the platform default in either direction.
    private Boolean enabled = null;
    private int port = DEFAULT_PORT;
    private String livenessPath = DEFAULT_LIVENESS_PATH;
    private String readinessPath = DEFAULT_READINESS_PATH;
    private String startupPath = DEFAULT_STARTUP_PATH;

    /**
     * Creates a health configuration from the {@code health} JSON section (or {@code null} when the
     * section is absent, yielding all defaults with {@code enabled} unset).
     *
     * @param jsonConfig the {@code health} config object, or {@code null}
     */
    public HealthConfiguration(JsonObject jsonConfig)
    {
        if (jsonConfig != null)
        {
            if (jsonConfig.has("enabled"))
            {
                enabled = jsonConfig.get("enabled").getAsBoolean();
            }
            if (jsonConfig.has("port"))
            {
                int p = jsonConfig.get("port").getAsInt();
                // The schema enforces 1..65535, but be defensive against a hand-edited config.
                if (p >= 1 && p <= 65535)
                {
                    port = p;
                }
                else
                {
                    LOGGER.warn("health.port {} out of range [1,65535]; using default {}", p, DEFAULT_PORT);
                }
            }
            if (jsonConfig.has("livenessPath"))
            {
                livenessPath = jsonConfig.get("livenessPath").getAsString();
            }
            if (jsonConfig.has("readinessPath"))
            {
                readinessPath = jsonConfig.get("readinessPath").getAsString();
            }
            if (jsonConfig.has("startupPath"))
            {
                startupPath = jsonConfig.get("startupPath").getAsString();
            }
        }
    }

    /**
     * Whether the {@code health.enabled} key was explicitly present in the config. When {@code true},
     * {@link #isEnabled()} wins over the platform-profile default; when {@code false}, the platform
     * default applies ({@code true} on KUBERNETES, {@code false} elsewhere).
     *
     * @return {@code true} if the config supplied an explicit {@code enabled} value
     */
    public boolean isEnabledExplicitlySet()
    {
        return enabled != null;
    }

    /**
     * The explicit {@code health.enabled} value, or {@code false} when the key was absent. Callers
     * deciding whether to start the server should first consult {@link #isEnabledExplicitlySet()}.
     *
     * @return the explicit enabled flag (or {@code false} if unset)
     */
    public boolean isEnabled()
    {
        return enabled != null && enabled;
    }

    /** @return the TCP port the health server binds (default {@value #DEFAULT_PORT}). */
    public int getPort()
    {
        return port;
    }

    /** @return the liveness probe path (default {@value #DEFAULT_LIVENESS_PATH}). */
    public String getLivenessPath()
    {
        return livenessPath;
    }

    /** @return the readiness probe path (default {@value #DEFAULT_READINESS_PATH}). */
    public String getReadinessPath()
    {
        return readinessPath;
    }

    /** @return the startup probe path (default {@value #DEFAULT_STARTUP_PATH}). */
    public String getStartupPath()
    {
        return startupPath;
    }
}
