/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.commands;

import com.google.gson.JsonObject;

import java.util.Objects;

/**
 * Explicit outcome of an {@link OutcomeCommandHandler}. Immediate outcomes retain the standard
 * command wrapper; a deferred outcome carries only an opaque inbox-issued reply handle and
 * suppresses automatic reply after the handler returns.
 */
public sealed interface CommandOutcome permits CommandOutcome.ImmediateSuccess,
        CommandOutcome.ImmediateError, CommandOutcome.Deferred {

    /** Immediate standard success; a {@code null} result becomes an empty acknowledgement. */
    record ImmediateSuccess(JsonObject result) implements CommandOutcome { }

    /** Immediate standard coded error. */
    record ImmediateError(String code, String message) implements CommandOutcome {
        public ImmediateError {
            if (code == null || code.isEmpty()) {
                throw new IllegalArgumentException("immediate error code must be non-empty");
            }
            message = message == null ? "" : message;
        }
    }

    /**
     * Deferred settlement through the issuing inbox. The token contains no topic or direct
     * publish capability and is valid only for the request from which it was provisioned.
     */
    record Deferred(CommandInbox.DeferredReply token, Runnable postAcceptContinuation)
            implements CommandOutcome {
        /** Preserves the established deferred outcome with no post-accept continuation. */
        public Deferred(CommandInbox.DeferredReply token) {
            this(token, null);
        }

        public Deferred {
            Objects.requireNonNull(token, "deferred token must not be null");
        }
    }

    /** Convenience factory for an immediate success. */
    static ImmediateSuccess success(JsonObject result) {
        return new ImmediateSuccess(result);
    }

    /** Convenience factory for an immediate coded error. */
    static ImmediateError error(String code, String message) {
        return new ImmediateError(code, message);
    }

    /** Convenience factory for an activated deferred reply. */
    static Deferred deferred(CommandInbox.DeferredReply token) {
        return new Deferred(token);
    }

    /**
     * Returns a deferred result whose continuation starts only after the inbox validates the
     * exact activated token for this delivery. The continuation owns no reply target; it must
     * settle the captured token through its normal guarded API.
     */
    static Deferred deferredWithContinuation(CommandInbox.DeferredReply token,
                                             Runnable postAcceptContinuation) {
        Objects.requireNonNull(postAcceptContinuation,
                "post-accept continuation must not be null");
        return new Deferred(token, postAcceptContinuation);
    }
}
