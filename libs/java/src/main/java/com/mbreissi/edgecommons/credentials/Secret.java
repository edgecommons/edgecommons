package com.mbreissi.edgecommons.credentials;

import java.nio.charset.CharacterCodingException;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.util.Map;

import com.google.gson.Gson;
import com.google.gson.JsonElement;

/**
 * A decrypted secret value plus its metadata. {@link #toString()} redacts the value — never log or
 * serialize it.
 */
public final class Secret {
    private static final Gson GSON = new Gson();

    private final String name;
    private final String version;
    private final byte[] value;
    private final Map<String, String> labels;
    private final long createdMs;
    private final String source;
    private final String contentType;

    Secret(String name, String version, byte[] value, Map<String, String> labels, long createdMs,
           String source, String contentType) {
        this.name = name;
        this.version = version;
        this.value = value;
        this.labels = labels;
        this.createdMs = createdMs;
        this.source = source;
        this.contentType = contentType;
    }

    public String name() {
        return name;
    }

    public String version() {
        return version;
    }

    public long createdMs() {
        return createdMs;
    }

    public String source() {
        return source;
    }

    public String contentType() {
        return contentType;
    }

    public Map<String, String> labels() {
        return labels;
    }

    /** The raw secret bytes. */
    public byte[] bytes() {
        return value;
    }

    /** The value as UTF-8 (throws if not valid UTF-8). */
    public String asString() {
        try {
            return StandardCharsets.UTF_8.newDecoder()
                    .onMalformedInput(CodingErrorAction.REPORT)
                    .onUnmappableCharacter(CodingErrorAction.REPORT)
                    .decode(java.nio.ByteBuffer.wrap(value))
                    .toString();
        } catch (CharacterCodingException e) {
            throw new CredentialException("secret is not valid UTF-8");
        }
    }

    /** The value parsed as JSON. */
    public JsonElement asJson() {
        try {
            return GSON.fromJson(asString(), JsonElement.class);
        } catch (RuntimeException e) {
            throw new CredentialException("secret is not JSON: " + e.getMessage());
        }
    }

    @Override
    public String toString() {
        return "Secret{name=" + name + ", version=" + version + ", bytes=<" + value.length + " redacted>}";
    }
}
