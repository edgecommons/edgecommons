/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.commands;

import com.mbreissi.edgecommons.messaging.Message;

/** A command handler that explicitly chooses immediate success/error or deferred settlement. */
@FunctionalInterface
public interface OutcomeCommandHandler {

    /**
     * Handles one validated command request.
     *
     * @param request full received command envelope
     * @return a non-null explicit outcome
     * @throws Exception a handler failure, mapped to the standard {@code HANDLER_ERROR}
     */
    CommandOutcome handle(Message request) throws Exception;
}
