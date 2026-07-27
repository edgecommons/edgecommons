/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.commands;

import com.mbreissi.edgecommons.messaging.Message;
import com.google.gson.JsonObject;

/**
 * A scope-aware command-verb handler ({@link CommandInbox#registerScoped}): invoked with
 * everything a {@link CommandHandler} receives PLUS the <b>addressed instance</b> — the
 * {@code {instance}} token of the delivery topic when the command was addressed instance-scope
 * ({@code ecv1/{device}/{component}/{instance}/cmd/{verb}}), or {@code null} when it was
 * addressed component-scope ({@code ecv1/{device}/{component}/cmd/{verb}}, D‑U28). The token is
 * parsed from the delivery topic against the inbox's own subscribed filters — a handler never
 * re-derives identity from config.
 *
 * <p>Result and failure semantics are identical to {@link CommandHandler}: the returned object is
 * wrapped into {@code {"ok": true, "result": ...}} (a {@code null} return is an empty result), a
 * {@link CommandException} keeps its code, and any other exception maps to
 * {@link CommandInbox#ERR_HANDLER_ERROR}. Handlers run synchronously on the messaging delivery
 * thread — keep them fast, or hand off internally.
 */
@FunctionalInterface
public interface ScopedCommandHandler {

    /**
     * Handles one command request.
     *
     * @param request           the full request envelope (body = the verb's arguments object; the
     *                          requester's {@code identity}/{@code tags}, when present, are
     *                          informational)
     * @param addressedInstance the delivery topic's instance token, or {@code null} when the
     *                          command was addressed component-scope
     * @return the verb-specific result object (may be {@code null} for an empty result)
     * @throws Exception any failure — a {@link CommandException} keeps its code, everything else
     *                   maps to {@link CommandInbox#ERR_HANDLER_ERROR}
     */
    JsonObject handle(Message request, String addressedInstance) throws Exception;
}
