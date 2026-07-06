package com.mbreissi.edgecommons.credentials;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.charset.StandardCharsets;
import java.util.Arrays;

import org.junit.jupiter.api.Test;

/**
 * Unit tests for {@link VaultCrypto} primitives: AEAD seal/open, HMAC, constant-time verify,
 * HKDF-SHA256 and the derived MAC key. Crypto correctness is also pinned cross-language in
 * {@link VaultTest#crossLanguageConformance()}; this class adds focused primitive coverage and
 * error paths.
 */
class VaultCryptoTest {

    private static byte[] key32() {
        byte[] k = new byte[VaultCrypto.KEY_LEN];
        for (int i = 0; i < k.length; i++) {
            k[i] = (byte) (0x40 + i);
        }
        return k;
    }

    private static byte[] nonce12() {
        byte[] n = new byte[VaultCrypto.NONCE_LEN];
        for (int i = 0; i < n.length; i++) {
            n[i] = (byte) (0xB0 + i);
        }
        return n;
    }

    @Test
    void randomReturnsRequestedLengthAndVaries() {
        byte[] a = VaultCrypto.random(32);
        byte[] b = VaultCrypto.random(32);
        assertEquals(32, a.length);
        assertEquals(32, b.length);
        assertFalse(Arrays.equals(a, b), "two random draws should differ with overwhelming probability");
        assertEquals(0, VaultCrypto.random(0).length);
    }

    @Test
    void sealOpenRoundTrip() {
        byte[] key = key32();
        byte[] nonce = nonce12();
        byte[] aad = "the-aad".getBytes(StandardCharsets.UTF_8);
        byte[] pt = "hello world".getBytes(StandardCharsets.UTF_8);

        byte[] ct = VaultCrypto.seal(key, nonce, aad, pt);
        // ciphertext||tag: GCM appends a 16-byte tag, so it is longer than the plaintext.
        assertEquals(pt.length + 16, ct.length);
        assertFalse(Arrays.equals(pt, Arrays.copyOf(ct, pt.length)));

        byte[] roundtrip = VaultCrypto.open(key, nonce, aad, ct);
        assertArrayEquals(pt, roundtrip);
    }

    @Test
    void openFailsOnWrongKey() {
        byte[] nonce = nonce12();
        byte[] aad = "aad".getBytes(StandardCharsets.UTF_8);
        byte[] ct = VaultCrypto.seal(key32(), nonce, aad, "secret".getBytes());

        byte[] wrong = new byte[VaultCrypto.KEY_LEN];
        Arrays.fill(wrong, (byte) 0x99);
        CredentialException ex = assertThrows(CredentialException.class,
                () -> VaultCrypto.open(wrong, nonce, aad, ct));
        assertTrue(ex.getMessage().contains("AEAD open failed"));
    }

    @Test
    void openFailsOnAadMismatch() {
        byte[] nonce = nonce12();
        byte[] ct = VaultCrypto.seal(key32(), nonce, "aad-a".getBytes(), "secret".getBytes());
        assertThrows(CredentialException.class,
                () -> VaultCrypto.open(key32(), nonce, "aad-b".getBytes(), ct));
    }

    @Test
    void openFailsOnTamperedCiphertext() {
        byte[] nonce = nonce12();
        byte[] aad = "aad".getBytes();
        byte[] ct = VaultCrypto.seal(key32(), nonce, aad, "secret".getBytes());
        ct[0] ^= 0x01;
        assertThrows(CredentialException.class, () -> VaultCrypto.open(key32(), nonce, aad, ct));
    }

    @Test
    void sealFailsOnWrongKeyLength() {
        // AES requires a 16/24/32-byte key; a 5-byte key is rejected by JCE → CredentialException.
        assertThrows(CredentialException.class,
                () -> VaultCrypto.seal(new byte[5], nonce12(), new byte[0], "x".getBytes()));
    }

    @Test
    void hmacIsDeterministicAndKeyed() {
        byte[] data = "message".getBytes(StandardCharsets.UTF_8);
        byte[] k1 = key32();
        byte[] mac1 = VaultCrypto.hmac(k1, data);
        byte[] mac2 = VaultCrypto.hmac(k1, data);
        assertArrayEquals(mac1, mac2, "HMAC must be deterministic for the same key+data");
        assertEquals(32, mac1.length); // SHA-256 output

        byte[] k2 = new byte[VaultCrypto.KEY_LEN];
        Arrays.fill(k2, (byte) 1);
        assertFalse(Arrays.equals(mac1, VaultCrypto.hmac(k2, data)), "different key → different HMAC");
    }

    @Test
    void hmacVerifyTrueAndFalse() {
        byte[] key = key32();
        byte[] data = "abc".getBytes();
        byte[] expected = VaultCrypto.hmac(key, data);
        assertTrue(VaultCrypto.hmacVerify(key, data, expected));

        byte[] bad = expected.clone();
        bad[0] ^= 0x01;
        assertFalse(VaultCrypto.hmacVerify(key, data, bad));
        assertFalse(VaultCrypto.hmacVerify(key, "abd".getBytes(), expected));
    }

    @Test
    void hkdfMatchesRfc5869BasicVector() {
        // RFC 5869 Appendix A.1 (SHA-256) test case 1.
        byte[] ikm = hex("0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b");
        byte[] salt = hex("000102030405060708090a0b0c");
        byte[] info = hex("f0f1f2f3f4f5f6f7f8f9");
        byte[] okm = VaultCrypto.hkdfSha256(ikm, salt, info, 42);
        byte[] expected = hex(
                "3cb25f25faacd57a90434f64d0362f2a"
                + "2d2d0a90cf1a5a4c5db02d56ecc4c5bf"
                + "34007208d5b887185865");
        assertArrayEquals(expected, okm);
    }

    @Test
    void hkdfUsesZeroSaltWhenSaltNullOrEmpty() {
        byte[] ikm = "ikm".getBytes();
        byte[] info = "info".getBytes();
        byte[] withNull = VaultCrypto.hkdfSha256(ikm, null, info, 32);
        byte[] withEmpty = VaultCrypto.hkdfSha256(ikm, new byte[0], info, 32);
        byte[] withZeros = VaultCrypto.hkdfSha256(ikm, new byte[32], info, 32);
        assertArrayEquals(withNull, withEmpty, "null and empty salt must behave the same");
        assertArrayEquals(withNull, withZeros, "empty salt must be treated as 32 zero bytes");
    }

    @Test
    void hkdfHonorsRequestedLength() {
        byte[] ikm = "ikm".getBytes();
        assertEquals(16, VaultCrypto.hkdfSha256(ikm, "s".getBytes(), "i".getBytes(), 16).length);
        // length spanning multiple HMAC blocks (> 32 bytes) exercises the expand loop.
        assertEquals(64, VaultCrypto.hkdfSha256(ikm, "s".getBytes(), "i".getBytes(), 64).length);
    }

    @Test
    void deriveMacKeyIsDeterministicAndBindsVaultId() {
        byte[] dek = key32();
        byte[] vaultA = "vault-a".getBytes(StandardCharsets.UTF_8);
        byte[] vaultB = "vault-b".getBytes(StandardCharsets.UTF_8);
        byte[] mk1 = VaultCrypto.deriveMacKey(dek, vaultA);
        byte[] mk2 = VaultCrypto.deriveMacKey(dek, vaultA);
        assertArrayEquals(mk1, mk2);
        assertEquals(VaultCrypto.KEY_LEN, mk1.length);
        assertFalse(Arrays.equals(mk1, VaultCrypto.deriveMacKey(dek, vaultB)),
                "different vaultId salt must yield a different MAC key");
    }

    @Test
    void differentNonceProducesDifferentCiphertext() {
        byte[] key = key32();
        byte[] aad = "aad".getBytes();
        byte[] pt = "secret".getBytes();
        byte[] n1 = nonce12();
        byte[] n2 = nonce12();
        n2[0] ^= 0x01;
        assertNotEquals(
                java.util.Base64.getEncoder().encodeToString(VaultCrypto.seal(key, n1, aad, pt)),
                java.util.Base64.getEncoder().encodeToString(VaultCrypto.seal(key, n2, aad, pt)));
    }

    private static byte[] hex(String s) {
        byte[] out = new byte[s.length() / 2];
        for (int i = 0; i < out.length; i++) {
            out[i] = (byte) Integer.parseInt(s.substring(2 * i, 2 * i + 2), 16);
        }
        return out;
    }
}
