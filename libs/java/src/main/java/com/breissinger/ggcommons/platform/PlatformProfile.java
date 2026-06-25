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
 * <p>Phase 0 carries only the two defaultable settings the resolver actually injects — the default
 * messaging {@link #transport() transport} and the default {@link #configSource() config source}.
 * Later phases append metrics/logging/credentials/streaming/identity defaults as additional fields
 * (additive; no resolver change). See DESIGN-core §3 for the full target table.
 *
 * @param transport    the default messaging transport for this platform
 * @param configSource the default {@code -c/--config} source token (e.g. {@code "GG_CONFIG"},
 *                     {@code "FILE"}) used when {@code -c} is omitted
 */
public record PlatformProfile(Transport transport, String configSource) {
}
