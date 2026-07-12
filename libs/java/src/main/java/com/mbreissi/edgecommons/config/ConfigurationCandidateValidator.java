/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

import com.google.gson.JsonObject;

import java.util.Objects;
import java.util.regex.Pattern;

/**
 * Side-effect-free pre-commit validation of a configuration candidate.
 *
 * <p>The candidate and prior snapshot are defensive copies. The prior snapshot is redacted and
 * is {@code null} for {@link ConfigurationValidationPhase#INITIAL}. A validator must not publish,
 * change sessions, or treat either object as live configuration. Throwing is treated as a stable
 * {@code VALIDATOR_FAILED} rejection; exceeding the configured deadline is a
 * {@code VALIDATION_TIMEOUT} rejection.
 */
@FunctionalInterface
public interface ConfigurationCandidateValidator {
    /** Validate one candidate without mutating runtime state. */
    Result validate(JsonObject candidate, JsonObject redactedCurrent,
                    ConfigurationValidationPhase phase) throws Exception;

    /** One validator's deterministic accept/reject verdict. */
    record Result(boolean accepted, String code, String message) {
        private static final Pattern CODE = Pattern.compile("^[A-Z][A-Z0-9_]{0,63}$");

        public Result {
            code = code == null ? "" : code;
            message = message == null ? "" : message;
            if (accepted) {
                code = "";
                message = "";
            } else if (!CODE.matcher(code).matches()) {
                throw new IllegalArgumentException(
                        "configuration validator rejection code must be stable SCREAMING_SNAKE_CASE");
            }
        }

        /** Accept the candidate. */
        public static Result accept() {
            return new Result(true, "", "");
        }

        /** Reject the candidate with a stable machine-readable code and safe diagnostic. */
        public static Result reject(String code, String message) {
            return new Result(false, Objects.requireNonNull(code, "code must not be null"), message);
        }
    }
}
