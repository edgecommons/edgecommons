/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging;

import com.aws.proserve.ggcommons.ParsedCommandLine;

/**
 * Builder for creating MessagingClient instances with proper dependency injection.
 */
public class MessagingClientBuilder {
    private ParsedCommandLine parsedCommandLine;
    private boolean receiveOwnMessages = true;
    
    private MessagingClientBuilder(ParsedCommandLine parsedCommandLine) {
        this.parsedCommandLine = parsedCommandLine;
    }
    
    public static MessagingClientBuilder create(ParsedCommandLine parsedCommandLine) {
        return new MessagingClientBuilder(parsedCommandLine);
    }
    
    public MessagingClientBuilder withReceiveOwnMessages(boolean receiveOwnMessages) {
        this.receiveOwnMessages = receiveOwnMessages;
        return this;
    }
    
    public MessagingClient build() {
        return new MessagingClient(parsedCommandLine, receiveOwnMessages);
    }
}