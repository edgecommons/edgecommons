/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.commands;

import com.mbreissi.edgecommons.messaging.Message;
import com.google.gson.JsonObject;

/**
 * A command-verb handler (DESIGN-uns §9.5): invoked by the {@link CommandInbox} for every
 * well-formed {@code cmd} envelope whose verb matches the registration.
 *
 * <p>The return value is the verb-specific <b>result object</b>, wrapped by the inbox into the
 * success reply body {@code {"ok": true, "result": <returned object>}} and published to the
 * request's {@code header.reply_to} (with the request's {@code correlation_id}). Returning
 * {@code null} yields an empty result ({@code {"ok": true, "result": {}}} — a plain
 * acknowledgement). When the request carries no {@code reply_to} (fire-and-forget) the handler
 * still runs but the result is discarded.
 *
 * <p>Failures: throw a {@link CommandException} for a coded error reply
 * ({@code {"ok": false, "error": {"code", "message"}}}); any other exception becomes the generic
 * {@link CommandInbox#ERR_HANDLER_ERROR} code. Handlers run synchronously on the messaging
 * delivery thread — keep them fast, or hand off internally.
 */
@FunctionalInterface
public interface CommandHandler {

    /**
     * Handles one command request.
     *
     * @param request the full request envelope (body = the verb's arguments object; the
     *                requester's {@code identity}/{@code tags}, when present, are informational)
     * @return the verb-specific result object (may be {@code null} for an empty result)
     * @throws Exception any failure — a {@link CommandException} keeps its code, everything else
     *                   maps to {@link CommandInbox#ERR_HANDLER_ERROR}
     */
    JsonObject handle(Message request) throws Exception;
}
