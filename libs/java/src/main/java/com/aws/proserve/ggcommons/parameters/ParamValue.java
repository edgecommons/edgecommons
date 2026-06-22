package com.aws.proserve.ggcommons.parameters;

import java.nio.charset.StandardCharsets;
import java.util.Optional;

/**
 * A parameter value fetched from a {@link ParameterSource}. {@code secure} values (an SSM
 * {@code SecureString}, a {@code mountedDir} secret path, …) must never be logged. Mirrors the Rust
 * {@code ParamValue}.
 */
public final class ParamValue {
    private final byte[] value;
    private final boolean secure;
    private final Optional<String> version;

    /**
     * @param value   raw value bytes (UTF-8 for SSM / env / text files)
     * @param secure  whether this value is sensitive (don't log; cache encrypted)
     * @param version upstream version for change detection on refresh ({@code null} if the source has none)
     */
    public ParamValue(byte[] value, boolean secure, String version) {
        this.value = value;
        this.secure = secure;
        this.version = Optional.ofNullable(version);
    }

    /** Construct a non-secure value with no upstream version. */
    public static ParamValue plain(byte[] value) {
        return new ParamValue(value, false, null);
    }

    /** Construct a non-secure value from a UTF-8 string. */
    public static ParamValue plain(String value) {
        return plain(value.getBytes(StandardCharsets.UTF_8));
    }

    public byte[] value() {
        return value;
    }

    public boolean secure() {
        return secure;
    }

    public Optional<String> version() {
        return version;
    }

    @Override
    public String toString() {
        return "ParamValue{bytes=<" + value.length + (secure ? " redacted (secure)" : "") + ">, secure=" + secure
                + ", version=" + version.orElse("") + "}";
    }
}
