/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.streaming;

/**
 * Raised when a native {@code edgestreamlog} C-ABI call returns a non-zero status. The {@link #code()}
 * mirrors the {@code esl_status} enum in {@code edgestreamlog.h}.
 */
public class EdgeStreamException extends RuntimeException {

    /** {@code esl_status} codes (must match edgestreamlog.h). */
    public static final int OK = 0;
    public static final int ERR_CONFIG = 1;
    public static final int ERR_IO = 2;
    public static final int ERR_CORRUPT = 3;
    public static final int ERR_FULL = 4;
    public static final int ERR_UNKNOWN_STREAM = 5;
    public static final int ERR_SINK = 6;
    public static final int ERR_PANIC = 7;
    public static final int ERR_INVALID_ARG = 8;

    private final int code;

    public EdgeStreamException(int code, String message) {
        super("edgestreamlog error " + code + (message == null ? "" : ": " + message));
        this.code = code;
    }

    /** The {@code esl_status} code returned by the native call. */
    public int code() {
        return code;
    }
}
