/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.messaging;

import com.google.gson.JsonObject;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.util.Objects;

/**
 * The privileged internal-publish seam (UNS-CANONICAL-DESIGN §4.2, D-U4): publishes that BYPASS
 * the reserved-class publish guard, obtained via {@link MessagingClient#reservedPublisher()}.
 *
 * <p><b>Library-internal.</b> This class is public only because the library's own publishers —
 * the heartbeat {@code state} keepalive, the {@code Messaging} metric target and the effective-
 * config ({@code cfg}) publisher — live in other packages and must reach it. Component code
 * should never use it: the guard it bypasses exists to keep the library-owned UNS classes
 * ({@code state | metric | cfg | log}) consistent fleet-wide. In-process bypass is possible by
 * design — the guard is misuse prevention, not a security boundary (broker ACLs are).
 */
public final class ReservedPublisher {

    private final MessagingClient client;

    /** Package-private: created only by {@link MessagingClient#reservedPublisher()}. */
    ReservedPublisher(MessagingClient client) {
        this.client = Objects.requireNonNull(client, "client must not be null");
    }

    /**
     * Publishes a message to a local/IPC topic without the reserved-class guard.
     *
     * @param topic the topic to publish to (typically a reserved UNS topic)
     * @param msg   the message to publish
     */
    public void publish(String topic, Message msg) {
        client.publishReserved(topic, msg);
    }

    /**
     * Publishes a raw JSON object to a local/IPC topic without the reserved-class guard.
     *
     * @param topic   the topic to publish to
     * @param payload the raw JSON payload
     */
    public void publishRaw(String topic, JsonObject payload) {
        client.publishReservedRaw(topic, payload);
    }

    /**
     * Publishes a message to an AWS IoT Core topic without the reserved-class guard.
     *
     * @param topic the IoT Core topic to publish to
     * @param msg   the message to publish
     * @param qos   the delivery quality of service
     */
    public void publishToIoTCore(String topic, Message msg, QOS qos) {
        client.publishReservedToIoTCore(topic, msg, qos);
    }
}
