package com.aws.proserve.ggcommons.credentials;

import java.io.ByteArrayOutputStream;
import java.security.GeneralSecurityException;
import java.security.MessageDigest;
import java.security.SecureRandom;

import javax.crypto.Cipher;
import javax.crypto.Mac;
import javax.crypto.spec.GCMParameterSpec;
import javax.crypto.spec.SecretKeySpec;

/**
 * Vault cryptographic primitives — must match the Rust/Python references byte-for-byte.
 *
 * <p>AES-256-GCM (96-bit nonce, 128-bit tag appended by JCE), HKDF-SHA256 (implemented with
 * {@link Mac} so no preview KDF API is needed), HMAC-SHA256 with constant-time verify. See
 * {@code docs/CREDENTIALS.md} §4 and {@code vault-test-vectors/}.
 */
public final class VaultCrypto {
    public static final int KEY_LEN = 32;
    public static final int NONCE_LEN = 12;
    private static final int TAG_BITS = 128;
    private static final SecureRandom RNG = new SecureRandom();

    private VaultCrypto() {}

    /** {@code n} cryptographically secure random bytes. */
    public static byte[] random(int n) {
        byte[] b = new byte[n];
        RNG.nextBytes(b);
        return b;
    }

    /** AES-256-GCM seal; returns {@code ciphertext || tag}. */
    public static byte[] seal(byte[] key, byte[] nonce, byte[] aad, byte[] plaintext) {
        try {
            Cipher c = Cipher.getInstance("AES/GCM/NoPadding");
            c.init(Cipher.ENCRYPT_MODE, new SecretKeySpec(key, "AES"), new GCMParameterSpec(TAG_BITS, nonce));
            c.updateAAD(aad);
            return c.doFinal(plaintext);
        } catch (GeneralSecurityException e) {
            throw new CredentialException("AEAD seal failed", e);
        }
    }

    /** AES-256-GCM open of {@code ciphertext || tag}; throws (never returns plaintext) on failure. */
    public static byte[] open(byte[] key, byte[] nonce, byte[] aad, byte[] ctAndTag) {
        try {
            Cipher c = Cipher.getInstance("AES/GCM/NoPadding");
            c.init(Cipher.DECRYPT_MODE, new SecretKeySpec(key, "AES"), new GCMParameterSpec(TAG_BITS, nonce));
            c.updateAAD(aad);
            return c.doFinal(ctAndTag);
        } catch (GeneralSecurityException e) {
            throw new CredentialException("AEAD open failed (wrong key, tampered data, or AAD mismatch)");
        }
    }

    /** HMAC-SHA256 of {@code data} under {@code key}. */
    public static byte[] hmac(byte[] key, byte[] data) {
        try {
            Mac m = Mac.getInstance("HmacSHA256");
            m.init(new SecretKeySpec(key, "HmacSHA256"));
            return m.doFinal(data);
        } catch (GeneralSecurityException e) {
            throw new CredentialException("HMAC failed", e);
        }
    }

    /** Constant-time check that {@code HMAC-SHA256(key, data) == expected}. */
    public static boolean hmacVerify(byte[] key, byte[] data, byte[] expected) {
        return MessageDigest.isEqual(hmac(key, data), expected);
    }

    /**
     * Derive the vault MAC key: {@code HKDF-SHA256(ikm=dek, salt=vaultId, info="…/mac")}.
     */
    public static byte[] deriveMacKey(byte[] dek, byte[] vaultIdUtf8) {
        return hkdfSha256(dek, vaultIdUtf8, "ggcommons-vault/v1/mac".getBytes(java.nio.charset.StandardCharsets.UTF_8), KEY_LEN);
    }

    /** RFC 5869 HKDF-SHA256 (extract + expand) implemented with HMAC-SHA256. */
    static byte[] hkdfSha256(byte[] ikm, byte[] salt, byte[] info, int len) {
        byte[] effSalt = (salt == null || salt.length == 0) ? new byte[32] : salt;
        byte[] prk = hmac(effSalt, ikm); // extract
        ByteArrayOutputStream okm = new ByteArrayOutputStream();
        byte[] t = new byte[0];
        int counter = 1;
        try {
            while (okm.size() < len) {
                Mac m = Mac.getInstance("HmacSHA256");
                m.init(new SecretKeySpec(prk, "HmacSHA256"));
                m.update(t);
                m.update(info);
                m.update((byte) counter);
                t = m.doFinal();
                okm.writeBytes(t);
                counter++;
            }
        } catch (GeneralSecurityException e) {
            throw new CredentialException("HKDF failed", e);
        }
        byte[] out = new byte[len];
        System.arraycopy(okm.toByteArray(), 0, out, 0, len);
        return out;
    }
}
