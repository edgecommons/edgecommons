/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging;

/**
 * Functional interface for handling received messages.
 * This provides a cleaner abstraction than BiConsumer for message handling.
 */
@FunctionalInterface
public interface MessageHandler {
    /**
     * Handles a received message.
     * 
     * @param topic The topic the message was received on
     * @param message The received message
     */
    void handle(String topic, Message message);
}