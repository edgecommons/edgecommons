/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.messaging;

import java.util.function.BiConsumer;

/**
 * Functional interface for handling received messages.
 * This provides a cleaner, message-oriented abstraction over {@link BiConsumer}; because it
 * extends {@code BiConsumer<String, Message>}, a {@code MessageHandler} can be passed directly
 * to any messaging API that accepts a {@code BiConsumer}.
 */
@FunctionalInterface
public interface MessageHandler extends BiConsumer<String, Message> {
    /**
     * Handles a received message.
     *
     * @param topic The topic the message was received on
     * @param message The received message
     */
    void handle(String topic, Message message);

    /**
     * {@inheritDoc}
     *
     * <p>Delegates to {@link #handle(String, Message)} so a {@code MessageHandler} is usable
     * wherever a {@code BiConsumer<String, Message>} is expected.
     */
    @Override
    default void accept(String topic, Message message) {
        handle(topic, message);
    }
}