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
}
