/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

/**
 * MQTT Quality of Service level used by EdgeCommons messaging APIs.
 *
 * <p>The enum is transport-neutral: standalone MQTT accepts all three MQTT QoS levels, while
 * Greengrass IoT Core IPC accepts only {@link #AT_MOST_ONCE} and {@link #AT_LEAST_ONCE}.
 */
public enum Qos {
    AT_MOST_ONCE(0),
    AT_LEAST_ONCE(1),
    EXACTLY_ONCE(2);

    private final int mqttLevel;

    Qos(int mqttLevel) {
        this.mqttLevel = mqttLevel;
    }

    /** Returns the MQTT numeric QoS level: 0, 1, or 2. */
    public int mqttLevel() {
        return mqttLevel;
    }

    /** Converts a numeric MQTT QoS level into the corresponding enum value. */
    public static Qos fromMqttLevel(int mqttLevel) {
        return switch (mqttLevel) {
            case 0 -> AT_MOST_ONCE;
            case 1 -> AT_LEAST_ONCE;
            case 2 -> EXACTLY_ONCE;
            default -> throw new IllegalArgumentException("MQTT QoS must be 0, 1, or 2 (got " + mqttLevel + ")");
        };
    }
}
