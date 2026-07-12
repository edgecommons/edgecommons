/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

/**
 * A strict confirmed-publish failure. A thrown instance means delivery acknowledgement was not
 * observed; callers must treat the publication as unsuccessful or ambiguous and may retry the
 * exact same encoded envelope.
 */
public final class PublishConfirmationException extends RuntimeException {

    /** The stage that prevented a positive delivery acknowledgement. */
    public enum Reason {
        /** The acknowledgement did not arrive inside the caller's bounded timeout. */
        TIMEOUT,
        /** The transport disconnected, rejected, or otherwise failed the publish operation. */
        TRANSPORT_ERROR,
        /** The waiting thread was interrupted before acknowledgement. */
        INTERRUPTED
    }

    private final Reason reason;

    /**
     * Creates a confirmed-publish failure.
     *
     * @param reason the acknowledgement failure category
     * @param message operator-safe failure detail
     * @param cause the transport failure, when available
     */
    public PublishConfirmationException(Reason reason, String message, Throwable cause) {
        super(message, cause);
        this.reason = reason;
    }

    /** The acknowledgement failure category. */
    public Reason getReason() {
        return reason;
    }
}
