/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.platform;

/**
 * The messaging <em>transport</em> — the secondary runtime axis (DESIGN-core §2). Defaults from the
 * resolved {@link Platform} (GREENGRASS&rarr;IPC, HOST&rarr;MQTT) and is independently overridable,
 * but constrained: {@link #IPC} is valid only on {@link Platform#GREENGRASS} (the Nucleus provides
 * the IPC socket). See {@link PlatformResolver#validate}.
 */
public enum Transport {
    /** Greengrass Nucleus IPC (domain socket). Requires {@link Platform#GREENGRASS}. */
    IPC,
    /** Dual-MQTT (local broker + AWS IoT Core). The off-Nucleus transport. */
    MQTT
}
