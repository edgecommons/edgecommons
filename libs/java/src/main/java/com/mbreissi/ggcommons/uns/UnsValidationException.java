/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.uns;

/**
 * Thrown by the {@link Uns} topic builder/validator when a topic, filter component, or token
 * violates the UNS grammar (UNS-CANONICAL-DESIGN §2.2). Carries a machine-readable {@link Code}
 * so callers (and the cross-language {@code uns-test-vectors}) can assert the exact failure
 * without parsing the human-readable message. All four language libraries fail with the
 * identical code set.
 */
public class UnsValidationException extends IllegalArgumentException {

    /**
     * The machine-readable UNS validation failure codes (the exact §2.2 set, pinned in
     * {@code uns-test-vectors/topics.json} so all four languages fail identically).
     */
    public enum Code {
        /** A topic level / channel token / instance id is empty (or the whole topic is). */
        EMPTY_TOKEN,
        /** A token contains a blacklisted character: {@code / + # \} or a control character (U+0000–U+001F, U+007F). */
        BAD_CHAR,
        /** A token contains the path-traversal sequence {@code ..}. */
        TRAVERSAL,
        /** The topic exceeds the IoT Core depth limit (more than 7 {@code /} separators / 8 levels). */
        DEPTH_EXCEEDED,
        /** The topic exceeds the IoT Core publish limit of 256 UTF-8 bytes. */
        LENGTH_EXCEEDED,
        /** A channel was supplied for a leaf class ({@code state}, {@code cfg}). */
        CHANNEL_ON_LEAF,
        /** No channel was supplied for a channeled (non-leaf) class. */
        CHANNEL_REQUIRED,
        /** The topic does not start with the UNS root literal {@value Uns#ROOT}. */
        BAD_ROOT,
        /** The class position holds no token or a token outside the closed {@link UnsClass} set. */
        BAD_CLASS,
        /** {@link Uns#validate(String)} accepts only concrete topics: {@code +}/{@code #} are rejected. */
        WILDCARD_IN_TOPIC
    }

    private final Code code;

    /**
     * Creates a validation exception with a machine-readable code and a human-readable detail.
     *
     * @param code    the machine-readable failure code (non-null)
     * @param message the human-readable detail
     */
    public UnsValidationException(Code code, String message) {
        super("[" + code + "] " + message);
        this.code = code;
    }

    /** Returns the machine-readable failure code. */
    public Code getCode() {
        return code;
    }
}
