/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.commands;

import java.util.Objects;

/**
 * A coded command failure (DESIGN-uns §9.5): thrown by a {@link CommandHandler} to produce a
 * structured error reply {@code {"ok": false, "error": {"code": <code>, "message": <message>}}}
 * with a caller-chosen machine-readable code. Any <em>other</em> exception a handler throws is
 * mapped to the generic {@link CommandInbox#ERR_HANDLER_ERROR} code — this class exists so a
 * handler (built-in or custom) can distinguish its failure modes for the console
 * (e.g. {@link CommandInbox#ERR_RELOAD_FAILED}, {@link CommandInbox#ERR_NO_CONFIG}).
 */
public class CommandException extends Exception {

    /** The machine-readable error code carried in the error reply's {@code error.code}. */
    private final String code;

    /**
     * @param code    the machine-readable error code (non-null, non-empty; SCREAMING_SNAKE_CASE
     *                by convention — see the pinned base codes on {@link CommandInbox})
     * @param message the human-readable message carried in the error reply's {@code error.message}
     */
    public CommandException(String code, String message) {
        super(message);
        Objects.requireNonNull(code, "code must not be null");
        if (code.isEmpty()) {
            throw new IllegalArgumentException("code must not be empty");
        }
        this.code = code;
    }

    /** The machine-readable error code for the error reply's {@code error.code}. */
    public String getCode() {
        return code;
    }
}
