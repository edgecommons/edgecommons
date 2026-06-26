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
 * ({@code "json"} on KUBERNETES, the stdout-JSON sink; {@code null} elsewhere). Later phases append
 * metrics/credentials/streaming/identity defaults as additional fields (additive; no resolver
 * change). See DESIGN-core §3 for the full target table.
 *
 * @param transport     the default messaging transport for this platform
 * @param configSource  the default {@code -c/--config} source token (e.g. {@code "GG_CONFIG"},
 *                      {@code "FILE"}) used when {@code -c} is omitted
 * @param loggingFormat the default {@code logging.<lang>_format} token applied when the component
 *                      config omits {@code logging.java_format} — the middle tier of the
 *                      logging-format precedence (FR-RT-3). {@code "json"} selects the stdout-JSON
 *                      sink on KUBERNETES (FR-LOG-1); {@code null} keeps the library console/text
 *                      default (GREENGRASS / HOST). See {@link PlatformResolver#LOGGING_FORMAT_JSON}.
 */
public record PlatformProfile(Transport transport, String configSource, String loggingFormat) {
}
