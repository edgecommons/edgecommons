/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.commands;

import com.mbreissi.edgecommons.messaging.Message;

/**
 * A command handler that explicitly chooses immediate success/error or deferred settlement.
 * Like every registration form it receives the library-resolved <b>addressed instance</b>
 * (DESIGN-scoped-commands D-SC-1); token semantics are unchanged — the handler provisions and
 * activates its own {@link CommandInbox.DeferredReply} and returns a tagged
 * {@link CommandOutcome}.
 */
@FunctionalInterface
public interface OutcomeCommandHandler {

    /**
     * Handles one validated command request.
     *
     * @param request           full received command envelope
     * @param addressedInstance the library-resolved addressed instance (see
     *                          {@link CommandHandler#handle}): topic token, else body
     *                          {@code instance}, or {@code null}
     * @return a non-null explicit outcome
     * @throws Exception a handler failure, mapped to the standard {@code HANDLER_ERROR}
     */
    CommandOutcome handle(Message request, String addressedInstance) throws Exception;
}
