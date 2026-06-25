/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.platform;

/**
 * The deployment <em>platform</em> — the primary runtime axis (DESIGN-core §2/§3). A platform is a
 * named profile: a table of per-subsystem default providers/targets/sinks selected by
 * {@link PlatformResolver}. Orthogonal to {@link Transport}; only messaging-transport is
 * platform-coupled (via the IPC lock, {@link PlatformResolver#validate}).
 *
 * <p>Phase 0 populates only {@link #GREENGRASS} and {@link #HOST} (a behavior-preserving
 * re-expression of today's two modes). {@link #KUBERNETES} is declared but <em>not</em> wired —
 * selecting it fails fast until its profile ships in Phase 1.
 */
public enum Platform {
    /** On an AWS IoT Greengrass v2 Nucleus: IPC transport, Nucleus-managed config/identity. */
    GREENGRASS,
    /** A plain host (Kubernetes/Docker/bare host without a Nucleus): MQTT transport. */
    HOST,
    /** Kubernetes (declared for Phase 0; profile populated in Phase 1). */
    KUBERNETES
}
