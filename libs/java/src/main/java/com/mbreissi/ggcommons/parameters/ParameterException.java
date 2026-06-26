package com.mbreissi.ggcommons.parameters;

/**
 * Any parameter-subsystem failure (unknown source, bad config, source read error, non-typed value,
 * cache I/O). Mirrors {@link com.mbreissi.ggcommons.credentials.CredentialException} and the
 * Rust {@code GgError::Parameters}. Messages never include a {@code secure} parameter's value.
 */
public class ParameterException extends RuntimeException {
    public ParameterException(String message) {
        super(message);
    }

    public ParameterException(String message, Throwable cause) {
        super(message, cause);
    }
}
