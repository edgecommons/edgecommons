package com.breissinger.ggcommons.credentials;

import java.util.Base64;

import com.breissinger.ggcommons.credentials.VaultModel.KekInfo;

/**
 * KEK sourced from a raw 32-byte key, base64-encoded, read from an environment variable (typically a
 * mounted Kubernetes Secret) — the offline-capable software KEK and the KUBERNETES-platform default
 * (FR-CRED-3 / FR-CRED-6).
 *
 * <p>The envelope crypto is <b>cryptographically identical</b> to {@link FileKeyProvider} given the
 * same raw 32-byte KEK: this class wraps an internal {@link FileKeyProvider} delegate built from the
 * decoded KEK, so the AES-256-GCM DEK wrap/unwrap and the AAD ({@link VaultFormat#dekWrapAad}) come
 * from the exact same code path. The only differences are that {@link #providerId()} returns
 * {@code "env"} and the persisted {@link KekInfo#provider} is {@code "env"}; the wrapped-DEK bytes are
 * byte-for-byte identical to a {@code file} wrap of the same DEK under the same KEK (the
 * {@code provider} field is metadata and is not part of any AAD). A vault wrapped by this provider can
 * therefore be unwrapped by a {@link FileKeyProvider} holding the same raw KEK, and vice-versa.
 */
public final class EnvKeyProvider implements KeyProvider {

    /**
     * Default environment variable name holding the base64-encoded 32-byte KEK, used when the
     * {@code keyProvider.envVar} config field is absent. Shared with the Rust/Python/TS references.
     */
    public static final String DEFAULT_ENV_VAR = "GGCOMMONS_VAULT_KEK";

    private final FileKeyProvider delegate;

    /**
     * Construct directly from a raw 32-byte KEK. Crypto-identical to {@code new FileKeyProvider(kek)}.
     *
     * @param kek the raw KEK, exactly {@link VaultCrypto#KEY_LEN} bytes
     * @throws CredentialException if {@code kek} is not {@link VaultCrypto#KEY_LEN} bytes
     */
    public EnvKeyProvider(byte[] kek) {
        this.delegate = new FileKeyProvider(kek);
    }

    /**
     * Build from the base64 KEK held in environment variable {@code envVarName} (read from
     * {@link System#getenv(String)}).
     *
     * @param envVarName the environment variable name
     * @return the provider whose KEK is the decoded value of that variable
     * @throws CredentialException if the variable is unset/empty, not valid base64, or does not decode
     *                             to exactly {@link VaultCrypto#KEY_LEN} bytes
     */
    public static EnvKeyProvider fromEnv(String envVarName) {
        return fromEnvValue(envVarName, System.getenv(envVarName));
    }

    /**
     * Build from an explicit base64 value (the body of {@link #fromEnv(String)}); package-private so
     * tests can inject a value without mutating the process environment. {@code envVarName} is used
     * only for error messages.
     *
     * @param envVarName the source variable name (for diagnostics only)
     * @param b64        the base64-encoded KEK value, or {@code null}/empty if the variable is unset
     * @return the constructed provider
     * @throws CredentialException on unset/empty, invalid-base64, or wrong-length input
     */
    static EnvKeyProvider fromEnvValue(String envVarName, String b64) {
        if (b64 == null || b64.isEmpty()) {
            throw new CredentialException(
                    "env key provider: environment variable '" + envVarName + "' is unset or empty");
        }
        byte[] kek;
        try {
            // Tolerate a trailing newline (common when the value is sourced from a mounted file).
            kek = Base64.getDecoder().decode(b64.trim());
        } catch (IllegalArgumentException e) {
            throw new CredentialException(
                    "env key provider: environment variable '" + envVarName + "' is not valid base64");
        }
        if (kek.length != VaultCrypto.KEY_LEN) {
            throw new CredentialException("env key provider: decoded KEK from '" + envVarName
                    + "' must be " + VaultCrypto.KEY_LEN + " bytes, got " + kek.length);
        }
        return new EnvKeyProvider(kek);
    }

    @Override
    public String providerId() {
        return "env";
    }

    @Override
    public KekInfo wrapDek(String vaultId, byte[] dek) {
        // Identical crypto to FileKeyProvider; only the provider tag differs.
        KekInfo k = delegate.wrapDek(vaultId, dek);
        k.provider = "env";
        return k;
    }

    @Override
    public byte[] unwrapDek(String vaultId, KekInfo kek) {
        return delegate.unwrapDek(vaultId, kek);
    }
}
