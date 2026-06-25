package com.breissinger.ggcommons.credentials;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.attribute.PosixFileAttributeView;
import java.nio.file.attribute.PosixFilePermission;
import java.util.Base64;
import java.util.Set;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import com.breissinger.ggcommons.credentials.VaultModel.KekInfo;

/**
 * Unit tests for {@link FileKeyProvider}: KEK length validation, key-file load/generate, DEK
 * wrap/unwrap round-trip, vaultId AAD binding, the cross-language wrapped-DEK vector, and error
 * paths. No live infra.
 */
class FileKeyProviderTest {

    private static final Base64.Decoder B64D = Base64.getDecoder();
    private static final Base64.Encoder B64E = Base64.getEncoder();
    private static final String VAULT_ID = "00000000-0000-4000-8000-000000000001";

    private static byte[] kek() {
        byte[] k = new byte[VaultCrypto.KEY_LEN];
        for (int i = 0; i < k.length; i++) {
            k[i] = (byte) i; // matches vault-test-vectors kekB64 (0x00..0x1f)
        }
        return k;
    }

    @Test
    void providerIdIsFile() {
        assertEquals("file", new FileKeyProvider(new byte[32]).providerId());
    }

    @Test
    void rejectsWrongKekLength() {
        CredentialException ex = assertThrows(CredentialException.class,
                () -> new FileKeyProvider(new byte[16]));
        assertTrue(ex.getMessage().contains("KEK must be 32 bytes"));
    }

    @Test
    void wrapUnwrapRoundTrip() {
        FileKeyProvider p = new FileKeyProvider(kek());
        byte[] dek = VaultCrypto.random(VaultCrypto.KEY_LEN);
        KekInfo k = p.wrapDek(VAULT_ID, dek);

        assertEquals("file", k.provider);
        assertEquals("AES-256-GCM", k.alg);
        assertTrue(k.wrapNonce != null && !k.wrapNonce.isBlank());
        assertTrue(k.wrappedDek != null && !k.wrappedDek.isBlank());

        assertArrayEquals(dek, p.unwrapDek(VAULT_ID, k));
    }

    @Test
    void unwrapFailsUnderWrongKek() {
        KekInfo k = new FileKeyProvider(kek()).wrapDek(VAULT_ID, VaultCrypto.random(32));
        byte[] other = new byte[32];
        java.util.Arrays.fill(other, (byte) 0x99);
        assertThrows(CredentialException.class, () -> new FileKeyProvider(other).unwrapDek(VAULT_ID, k));
    }

    @Test
    void unwrapFailsUnderWrongVaultIdAad() {
        FileKeyProvider p = new FileKeyProvider(kek());
        KekInfo k = p.wrapDek(VAULT_ID, VaultCrypto.random(32));
        assertThrows(CredentialException.class, () -> p.unwrapDek("a-different-vault-id", k));
    }

    @Test
    void unwrapFailsWhenWrapNonceMissing() {
        KekInfo k = new KekInfo();
        k.wrappedDek = B64E.encodeToString(new byte[48]);
        k.wrapNonce = null;
        CredentialException ex = assertThrows(CredentialException.class,
                () -> new FileKeyProvider(kek()).unwrapDek(VAULT_ID, k));
        assertTrue(ex.getMessage().contains("missing wrapNonce"));
    }

    @Test
    void fromKeyFileLoadsRawBytes(@TempDir Path dir) throws Exception {
        Path kf = dir.resolve("kek.bin");
        Files.write(kf, kek());
        FileKeyProvider p = FileKeyProvider.fromKeyFile(kf);
        // round-trip proves the loaded KEK is the one written
        KekInfo k = p.wrapDek(VAULT_ID, new byte[32]);
        assertArrayEquals(new byte[32], p.unwrapDek(VAULT_ID, k));
    }

    @Test
    void fromKeyFileRejectsWrongLengthFile(@TempDir Path dir) throws Exception {
        Path kf = dir.resolve("short.bin");
        Files.write(kf, new byte[10]);
        assertThrows(CredentialException.class, () -> FileKeyProvider.fromKeyFile(kf));
    }

    @Test
    void fromKeyFileMissingFileFails(@TempDir Path dir) {
        CredentialException ex = assertThrows(CredentialException.class,
                () -> FileKeyProvider.fromKeyFile(dir.resolve("nope.bin")));
        assertTrue(ex.getMessage().contains("read key file"));
    }

    @Test
    void generateKeyFileWritesUsable32ByteKey(@TempDir Path dir) throws Exception {
        Path kf = dir.resolve("gen.key");
        FileKeyProvider p = FileKeyProvider.generateKeyFile(kf);
        assertTrue(Files.exists(kf));
        assertEquals(VaultCrypto.KEY_LEN, Files.readAllBytes(kf).length);

        // The returned provider must use the same key that was written.
        FileKeyProvider reloaded = FileKeyProvider.fromKeyFile(kf);
        KekInfo k = p.wrapDek(VAULT_ID, new byte[32]);
        assertArrayEquals(new byte[32], reloaded.unwrapDek(VAULT_ID, k));

        // POSIX-only: file must be 0600. Skipped silently on Windows.
        PosixFileAttributeView posix =
                Files.getFileAttributeView(kf, PosixFileAttributeView.class);
        if (posix != null) {
            Set<PosixFilePermission> perms = Files.getPosixFilePermissions(kf);
            assertEquals(Set.of(PosixFilePermission.OWNER_READ, PosixFilePermission.OWNER_WRITE), perms);
        }
    }

    @Test
    void generateKeyFileFailsWhenPathUnwritable(@TempDir Path dir) {
        // Parent directory does not exist → the underlying write throws IOException, wrapped.
        Path bad = dir.resolve("no-such-dir").resolve("gen.key");
        CredentialException ex = assertThrows(CredentialException.class,
                () -> FileKeyProvider.generateKeyFile(bad));
        assertTrue(ex.getMessage().contains("write key file"));
    }

    @Test
    void wrappedDekMatchesCrossLanguageVector() {
        // Independent of FileKeyProvider's random nonce: pin the dek-wrap AEAD against the vector.
        byte[] kek = B64D.decode("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=");
        byte[] dek = B64D.decode("QEFCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFlaW1xdXl8=");
        byte[] wrapNonce = B64D.decode("oKGio6Slpqeoqaqr");
        byte[] wrapped = VaultCrypto.seal(kek, wrapNonce, VaultFormat.dekWrapAad(VAULT_ID), dek);
        assertEquals("plk+bgGORPgqLM2YSzeOkSD9C0PG4hQ7xFd83SP2K14LigoLIHz/qr0Q+el+YrQf",
                B64E.encodeToString(wrapped));
    }
}
