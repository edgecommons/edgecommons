/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.platform;

/**
 * A platform profile: the table of per-subsystem <em>defaults</em> for a {@link Platform}
 * (DESIGN-core §3). Pure data; the {@link PlatformResolver} consults it only for settings the caller
 * did not set explicitly.
 *
 * <p>Phase 0 carried only the two defaultable settings the resolver actually injects — the default
 * messaging {@link #transport() transport} and the default {@link #configSource() config source}.
 * Phase 1c (FR-LOG-1/4, FR-RT-3) appends the default {@link #loggingFormat() logging-format} token
 * ({@code "json"} on KUBERNETES, the stdout-JSON sink; {@code null} elsewhere) and the
 * {@link #healthEnabled() health-server default} ({@code true} on KUBERNETES — the HTTP health
 * endpoint, FR-HB-1). Phase 1d appends the {@link #metricTarget() metric-target default}
 * ({@code "prometheus"} on KUBERNETES) and the {@link #credentialsKeyProvider() credentials
 * key-provider default} ({@code "env"} on KUBERNETES — the offline-capable software KEK, FR-CRED-6).
 * Later phases append streaming/identity defaults as additional fields (additive; no resolver change).
 * See DESIGN-core §3 for the full target table.
 *
 * @param transport     the default messaging transport for this platform
 * @param configSource  the default {@code -c/--config} source token (e.g. {@code "GG_CONFIG"},
 *                      {@code "FILE"}) used when {@code -c} is omitted
 * @param loggingFormat the default {@code logging.<lang>_format} token applied when the component
 *                      config omits {@code logging.java_format} — the middle tier of the
 *                      logging-format precedence (FR-RT-3). {@code "json"} selects the stdout-JSON
 *                      sink on KUBERNETES (FR-LOG-1); {@code null} keeps the library console/text
 *                      default (GREENGRASS / HOST). See {@link PlatformResolver#LOGGING_FORMAT_JSON}.
 * @param healthEnabled the default for the HTTP health server (FR-HB-1) when the config omits
 *                      {@code health.enabled} — the middle tier of the enablement precedence
 *                      (FR-RT-3). {@code true} on KUBERNETES (the server starts with no config
 *                      needed); {@code false} on GREENGRASS / HOST (opt-in via {@code health.enabled}).
 * @param metricTarget  the default {@code metricEmission.target} token applied when the component
 *                      config omits {@code metricEmission.target} — the middle tier of the metric-target
 *                      precedence (FR-RT-3). {@code "prometheus"} selects the pull-based in-process
 *                      registry + {@code /metrics} HTTP endpoint on KUBERNETES (FR-MET-1); {@code null}
 *                      keeps the library default {@code "log"} (GREENGRASS / HOST). See
 *                      {@link PlatformResolver#METRIC_TARGET_PROMETHEUS}.
 * @param credentialsKeyProvider the default {@code credentials.vault.keyProvider.type} token applied
 *                      when a {@code credentials} section is present but omits an explicit
 *                      {@code keyProvider.type} — the middle tier of the key-provider precedence
 *                      (FR-CRED-6 / FR-RT-3). {@code "env"} selects the {@link
 *                      com.breissinger.ggcommons.credentials.EnvKeyProvider env} provider (base64 KEK
 *                      from a mounted Secret) on KUBERNETES; {@code null} keeps the library default
 *                      {@code "file"} (GREENGRASS / HOST). It never auto-enables credentials — it only
 *                      changes the default provider type for an already-configured vault. See
 *                      {@link PlatformResolver#CREDENTIALS_KEY_PROVIDER_ENV}.
 */
public record PlatformProfile(Transport transport, String configSource, String loggingFormat,
                              boolean healthEnabled, String metricTarget, String credentialsKeyProvider) {
}
