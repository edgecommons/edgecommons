/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

/**
 * Thrown by the reserved-class publish guard (UNS-CANONICAL-DESIGN §4.1, D-U4/D-U8/D-U24) when a
 * client-chosen topic targets a library-owned UNS class ({@code state | metric | cfg | log},
 * {@link com.mbreissi.edgecommons.uns.UnsClass#RESERVED}). Components must not publish to reserved
 * classes directly — the library publishers (heartbeat/state keepalive, the metric subsystem, the
 * effective-config publisher) own those topics and reach them through the privileged
 * {@link ReservedPublisher} seam.
 *
 * <p>The guard is misuse prevention, not a security boundary — per-device broker ACLs are the
 * durable enforcement (DESIGN-uns §7.5).
 */
public class ReservedTopicException extends IllegalArgumentException {

    private final String topic;
    private final String classToken;

    /**
     * @param topic      the rejected client-chosen topic
     * @param classToken the reserved UNS class token found at the class position
     */
    public ReservedTopicException(String topic, String classToken) {
        super("topic '" + topic + "' targets the reserved UNS class '" + classToken
                + "' (state|metric|cfg|log are library-owned): use the library publishers instead"
                + " (heartbeat/state keepalive, the metric subsystem via gg.getMetrics(), the"
                + " effective-config publisher)");
        this.topic = topic;
        this.classToken = classToken;
    }

    /** The rejected topic. */
    public String getTopic() {
        return topic;
    }

    /** The reserved UNS class token ({@code state | metric | cfg | log}) that triggered the rejection. */
    public String getClassToken() {
        return classToken;
    }
}
