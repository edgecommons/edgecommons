package com.mbreissi.edgecommons.credentials;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.attribute.PosixFilePermission;
import java.util.Base64;
import java.util.EnumSet;

import com.mbreissi.edgecommons.credentials.VaultModel.KekInfo;

/**
 * KEK held as 32 bytes in a local key file (the standalone / offline-fallback custodian). The DEK
 * is wrapped with AES-256-GCM under the KEK, AAD-bound to the vault id — identical to the Rust and
 * Python references, so a vault wrapped by one language unwraps in another.
 */
public final class FileKeyProvider implements KeyProvider {
    private static final Base64.Encoder B64E = Base64.getEncoder();
    private static final Base64.Decoder B64D = Base64.getDecoder();

    private final byte[] kek;

    public FileKeyProvider(byte[] kek) {
        if (kek.length != VaultCrypto.KEY_LEN) {
            throw new CredentialException("KEK must be " + VaultCrypto.KEY_LEN + " bytes");
        }
        this.kek = kek.clone();
    }

    /** Load the KEK from a key file (exactly 32 raw bytes). */
    public static FileKeyProvider fromKeyFile(Path path) {
        try {
            return new FileKeyProvider(Files.readAllBytes(path));
        } catch (IOException e) {
            throw new CredentialException("read key file: " + e.getMessage(), e);
        }
    }

    /** Generate a fresh random KEK, write it to {@code path} (0600 where supported), and return it. */
    public static FileKeyProvider generateKeyFile(Path path) {
        byte[] kek = VaultCrypto.random(VaultCrypto.KEY_LEN);
        try {
            Files.write(path, kek);
            try {
                Files.setPosixFilePermissions(path, EnumSet.of(PosixFilePermission.OWNER_READ, PosixFilePermission.OWNER_WRITE));
            } catch (UnsupportedOperationException | IOException ignored) {
                // Non-POSIX (Windows): rely on directory ACLs.
            }
        } catch (IOException e) {
            throw new CredentialException("write key file: " + e.getMessage(), e);
        }
        return new FileKeyProvider(kek);
    }

    @Override
    public String providerId() {
        return "file";
    }

    @Override
    public KekInfo wrapDek(String vaultId, byte[] dek) {
        byte[] nonce = VaultCrypto.random(VaultCrypto.NONCE_LEN);
        byte[] wrapped = VaultCrypto.seal(kek, nonce, VaultFormat.dekWrapAad(vaultId), dek);
        KekInfo k = new KekInfo();
        k.provider = "file";
        k.alg = "AES-256-GCM";
        k.wrapNonce = B64E.encodeToString(nonce);
        k.wrappedDek = B64E.encodeToString(wrapped);
        return k;
    }

    @Override
    public byte[] unwrapDek(String vaultId, KekInfo kek) {
        if (kek.wrapNonce == null) {
            throw new CredentialException("file KEK: missing wrapNonce");
        }
        byte[] nonce = B64D.decode(kek.wrapNonce);
        byte[] wrapped = B64D.decode(kek.wrappedDek);
        return VaultCrypto.open(this.kek, nonce, VaultFormat.dekWrapAad(vaultId), wrapped);
    }
}
