package com.mbreissi.edgecommons.credentials;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.Base64;

import org.junit.jupiter.api.Test;

import com.mbreissi.edgecommons.credentials.VaultModel.KekInfo;

/**
 * Unit tests for {@link EnvKeyProvider}: base64 KEK parsing/validation from the environment, the
 * {@code env} provider id, DEK wrap/unwrap round-trip, the error paths (unset / invalid base64 /
 * wrong length), and — most importantly — <b>crypto-identity</b> with {@link FileKeyProvider} for the
 * same raw 32-byte KEK (a vault wrapped by one unwraps under the other). No live infra.
 */
class EnvKeyProviderTest {

    private static final Base64.Encoder B64E = Base64.getEncoder();
    private static final String VAULT_ID = "00000000-0000-4000-8000-000000000001";

    /** The 0x00..0x1f KEK, matching the shared cross-language vector. */
    private static byte[] kek() {
        byte[] k = new byte[VaultCrypto.KEY_LEN];
        for (int i = 0; i < k.length; i++) {
            k[i] = (byte) i;
        }
        return k;
    }

    private static String kekB64() {
        return B64E.encodeToString(kek());
    }

    @Test
    void providerIdIsEnv() {
        assertEquals("env", new EnvKeyProvider(kek()).providerId());
    }

    @Test
    void rejectsWrongKekLength() {
        // Constructor delegates to FileKeyProvider's length check.
        assertThrows(CredentialException.class, () -> new EnvKeyProvider(new byte[16]));
    }

    @Test
    void fromEnvValueDecodesBase32ByteKek() {
        EnvKeyProvider p = EnvKeyProvider.fromEnvValue("X", kekB64());
        KekInfo k = p.wrapDek(VAULT_ID, new byte[32]);
        assertEquals("env", k.provider);
        assertArrayEquals(new byte[32], p.unwrapDek(VAULT_ID, k));
    }

    @Test
    void fromEnvValueToleratesTrailingNewline() {
        // A value sourced from a mounted file may carry a trailing newline.
        EnvKeyProvider p = EnvKeyProvider.fromEnvValue("X", kekB64() + "\n");
        KekInfo k = p.wrapDek(VAULT_ID, new byte[32]);
        assertArrayEquals(new byte[32], p.unwrapDek(VAULT_ID, k));
    }

    @Test
    void wrapUnwrapRoundTrip() {
        EnvKeyProvider p = EnvKeyProvider.fromEnvValue("X", kekB64());
        byte[] dek = VaultCrypto.random(VaultCrypto.KEY_LEN);
        KekInfo k = p.wrapDek(VAULT_ID, dek);

        assertEquals("env", k.provider);
        assertEquals("AES-256-GCM", k.alg);
        assertTrue(k.wrapNonce != null && !k.wrapNonce.isBlank());
        assertTrue(k.wrappedDek != null && !k.wrappedDek.isBlank());

        assertArrayEquals(dek, p.unwrapDek(VAULT_ID, k));
    }

    // ---------- crypto-identity with FileKeyProvider (FR-CRED-3) ----------

    @Test
    void envWrappedDekUnwrapsUnderFileKeyProviderWithSameKek() {
        // Cross-custodian: wrap with env, unwrap with file, given the SAME raw KEK.
        EnvKeyProvider env = new EnvKeyProvider(kek());
        FileKeyProvider file = new FileKeyProvider(kek());
        byte[] dek = VaultCrypto.random(VaultCrypto.KEY_LEN);

        KekInfo wrappedByEnv = env.wrapDek(VAULT_ID, dek);
        assertArrayEquals(dek, file.unwrapDek(VAULT_ID, wrappedByEnv));

        KekInfo wrappedByFile = file.wrapDek(VAULT_ID, dek);
        assertArrayEquals(dek, env.unwrapDek(VAULT_ID, wrappedByFile));
    }

    @Test
    void envWrappedDekBytesAreIdenticalToFileSealForSameNonceAndKek() {
        // The wrapped-DEK bytes are a pure function of (KEK, nonce, AAD, DEK) — provider-independent.
        // Take a real env wrap, then re-seal the same DEK with the file-custodian KEK using the same
        // nonce the env provider chose: the ciphertext+tag must be byte-for-byte identical. Only the
        // KekInfo.provider tag ("env" vs "file") differs.
        byte[] dek = new byte[32];
        java.util.Arrays.fill(dek, (byte) 0x5a);

        KekInfo wrappedByEnv = new EnvKeyProvider(kek()).wrapDek(VAULT_ID, dek);
        assertEquals("env", wrappedByEnv.provider);

        byte[] nonce = Base64.getDecoder().decode(wrappedByEnv.wrapNonce);
        byte[] fileSeal = VaultCrypto.seal(kek(), nonce, VaultFormat.dekWrapAad(VAULT_ID), dek);

        assertEquals(wrappedByEnv.wrappedDek, B64E.encodeToString(fileSeal));
    }

    // ---------- error paths ----------

    @Test
    void fromEnvUnsetVariableFails() {
        // A name that is guaranteed not present in the environment.
        CredentialException ex = assertThrows(CredentialException.class,
                () -> EnvKeyProvider.fromEnv("EDGECOMMONS_DEFINITELY_UNSET_KEK_" + System.nanoTime()));
        assertTrue(ex.getMessage().contains("unset or empty"));
    }

    @Test
    void fromEnvValueEmptyFails() {
        CredentialException ex = assertThrows(CredentialException.class,
                () -> EnvKeyProvider.fromEnvValue("X", ""));
        assertTrue(ex.getMessage().contains("unset or empty"));
    }

    @Test
    void fromEnvValueNullFails() {
        CredentialException ex = assertThrows(CredentialException.class,
                () -> EnvKeyProvider.fromEnvValue("X", null));
        assertTrue(ex.getMessage().contains("unset or empty"));
    }

    @Test
    void fromEnvValueInvalidBase64Fails() {
        CredentialException ex = assertThrows(CredentialException.class,
                () -> EnvKeyProvider.fromEnvValue("X", "!!!not-base64!!!"));
        assertTrue(ex.getMessage().contains("not valid base64"));
    }

    @Test
    void fromEnvValueWrongLengthFails() {
        // Valid base64, but decodes to 16 bytes, not 32.
        String b64Of16 = B64E.encodeToString(new byte[16]);
        CredentialException ex = assertThrows(CredentialException.class,
                () -> EnvKeyProvider.fromEnvValue("X", b64Of16));
        assertTrue(ex.getMessage().contains("must be 32 bytes"));
    }
}
