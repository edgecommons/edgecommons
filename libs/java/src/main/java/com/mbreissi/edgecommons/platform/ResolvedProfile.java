/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.platform;

/**
 * The output of {@link PlatformResolver#resolveProfile}: the fully resolved runtime settings that
 * every subsystem initializer consumes (DESIGN-core §4). Produced once, right after argument parse
 * and before messaging init, from parse-time inputs only (flags &rarr; env &rarr; messaging-config
 * payload).
 *
 * @param platform            the resolved platform (after auto-detection / explicit flag)
 * @param transport           the resolved messaging transport (validated against the platform)
 * @param configSource        the resolved {@code -c/--config} argument vector (explicit, else the
 *                            profile default as a single-element array)
 * @param identity            the resolved IoT Thing name (identity), never {@code null}
 * @param messagingConfigPath the resolved MQTT messaging-config path (the explicit
 *                            {@code --transport MQTT <path>} payload, or — under CONFIGMAP+MQTT with
 *                            no explicit path — the default ConfigMap file path; FR-MSG-1).
 *                            {@code null} when no path applies.
 */
public record ResolvedProfile(Platform platform, Transport transport, String[] configSource,
                              String identity, String messagingConfigPath) {
}
