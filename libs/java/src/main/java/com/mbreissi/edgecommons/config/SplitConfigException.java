package com.mbreissi.edgecommons.config;

/**
 * Internal split-config failure with a stable code used by tests and CONFIG_COMPONENT errors.
 */
public class SplitConfigException extends RuntimeException {
    private final String code;

    public SplitConfigException(String code, String message) {
        super(code + ": " + message);
        this.code = code;
    }

    public SplitConfigException(String code, String message, Throwable cause) {
        super(code + ": " + message, cause);
        this.code = code;
    }

    public String getCode() {
        return code;
    }
}
