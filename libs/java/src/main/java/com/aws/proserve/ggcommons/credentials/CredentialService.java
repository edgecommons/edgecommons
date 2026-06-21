package com.aws.proserve.ggcommons.credentials;

import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Optional;

import com.google.gson.Gson;
import com.google.gson.JsonElement;

/**
 * The public credential interface (depend on this, not {@link DefaultCredentialService}).
 * Obtained from the runtime via {@code getCredentials()}.
 */
public interface CredentialService {
    Gson GSON = new Gson();

    /** Latest version of {@code name}, or empty. */
    Optional<Secret> get(String name);

    /** A specific version of {@code name}. */
    Optional<Secret> getVersion(String name, String version);

    /** Whether a secret exists. */
    boolean exists(String name);

    /** Metadata for all secrets under {@code prefix} ("" = all). Never returns values. */
    List<SecretMeta> list(String prefix);

    /** Retained version ids for {@code name} (oldest→newest). */
    List<String> versions(String name);

    /** Write a local secret version; returns the new version id. */
    String put(String name, byte[] value, PutOptions opts);

    /** Remove a secret entirely. */
    boolean delete(String name);

    /** Force an immediate pull from the central source (no-op without central sync). */
    default void refresh() {
    }

    /** Non-sensitive stats for observability (default: just the secret count). */
    default CredentialStats stats() {
        return new CredentialStats(list("").size(), null, 0, 0);
    }

    default String put(String name, byte[] value) {
        return put(name, value, PutOptions.defaults());
    }

    default Optional<byte[]> getBytes(String name) {
        return get(name).map(Secret::bytes);
    }

    default Optional<String> getString(String name) {
        return get(name).map(Secret::asString);
    }

    default Optional<JsonElement> getJson(String name) {
        return get(name).map(Secret::asJson);
    }

    default String putString(String name, String value) {
        return put(name, value.getBytes(StandardCharsets.UTF_8));
    }

    // ----- typed views (thin parses over the opaque secret; canonical camelCase JSON) -----

    default Optional<AwsCredentials> getAwsCredentials(String name) {
        return get(name).map(s -> {
            AwsCredentials v = parse(s, AwsCredentials.class, "AWS credentials");
            if (v.accessKeyId() == null || v.secretAccessKey() == null) {
                throw new CredentialException("secret '" + s.name() + "' is not AWS credentials (missing fields)");
            }
            return v;
        });
    }

    default Optional<BasicAuth> getBasicAuth(String name) {
        return get(name).map(s -> {
            BasicAuth v = parse(s, BasicAuth.class, "basic auth");
            if (v.username() == null || v.password() == null) {
                throw new CredentialException("secret '" + s.name() + "' is not basic auth (missing fields)");
            }
            return v;
        });
    }

    default Optional<TlsBundle> getTlsBundle(String name) {
        return get(name).map(s -> {
            TlsBundle v = parse(s, TlsBundle.class, "a TLS bundle");
            if (v.certPem() == null || v.keyPem() == null) {
                throw new CredentialException("secret '" + s.name() + "' is not a TLS bundle (missing fields)");
            }
            return v;
        });
    }

    default Optional<KafkaSasl> getKafkaSasl(String name) {
        return get(name).map(s -> {
            KafkaSasl v = parse(s, KafkaSasl.class, "Kafka SASL");
            if (v.username() == null || v.password() == null) {
                throw new CredentialException("secret '" + s.name() + "' is not Kafka SASL (missing fields)");
            }
            return v.mechanism() == null ? new KafkaSasl("PLAIN", v.username(), v.password()) : v;
        });
    }

    private static <T> T parse(Secret s, Class<T> cls, String kind) {
        try {
            T v = GSON.fromJson(s.asString(), cls);
            if (v == null) {
                throw new CredentialException("secret '" + s.name() + "' is not " + kind);
            }
            return v;
        } catch (com.google.gson.JsonParseException e) {
            throw new CredentialException("secret '" + s.name() + "' is not " + kind + ": " + e.getMessage());
        }
    }
}
