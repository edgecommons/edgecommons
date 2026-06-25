package com.breissinger.ggcommons.credentials;

import java.io.IOException;
import java.nio.channels.FileChannel;
import java.nio.channels.FileLock;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.nio.file.StandardOpenOption;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Base64;
import java.util.List;
import java.util.UUID;

import com.breissinger.ggcommons.credentials.VaultModel.KekInfo;
import com.breissinger.ggcommons.credentials.VaultModel.SecretEntry;
import com.breissinger.ggcommons.credentials.VaultModel.VaultFile;
import com.breissinger.ggcommons.credentials.VaultModel.VersionEntry;
import com.google.gson.Gson;
import com.google.gson.GsonBuilder;

/**
 * The encrypted local secret store (Java port of the Rust reference).
 *
 * <p>Single JSON file; AES-256-GCM records; envelope-wrapped DEK; HMAC over the canonical byte
 * string. Atomic temp→rename writes under a cross-process file lock for the shared device vault;
 * reload-on-change reads; fail-closed on a wrong KEK or tamper. Not internally synchronized — the
 * {@link DefaultCredentialService} serializes access.
 */
public final class LocalVault {
    private static final Gson GSON = new GsonBuilder().setPrettyPrinting().disableHtmlEscaping().create();
    private static final Base64.Encoder B64E = Base64.getEncoder();
    private static final Base64.Decoder B64D = Base64.getDecoder();

    private final Path path;
    private final String vaultId;
    private final byte[] dek;
    @SuppressWarnings("unused") // retained for phase-2 KEK rotation
    private final KeyProvider keyProvider;
    private KekInfo kek;
    private java.util.TreeMap<String, SecretEntry> secrets;
    private final int keep;
    private String stamp;

    private LocalVault(Path path, String vaultId, byte[] dek, KeyProvider keyProvider, KekInfo kek,
                       java.util.TreeMap<String, SecretEntry> secrets, int keep) {
        this.path = path;
        this.vaultId = vaultId;
        this.dek = dek;
        this.keyProvider = keyProvider;
        this.kek = kek;
        this.secrets = secrets;
        this.keep = Math.max(1, keep);
        this.stamp = fileStamp();
    }

    /** Open an existing vault or create a new empty one at {@code path}. */
    public static LocalVault open(Path path, KeyProvider keyProvider, int keepVersions) {
        if (Files.exists(path)) {
            VaultFile vf = readFile(path);
            if (vf.format != VaultFormat.FORMAT_VERSION) {
                throw new CredentialException("unsupported vault format " + vf.format);
            }
            byte[] dek = keyProvider.unwrapDek(vf.vaultId, vf.kek);
            verifyMac(dek, vf);
            java.util.TreeMap<String, SecretEntry> secrets = vf.secrets != null ? vf.secrets : new java.util.TreeMap<>();
            return new LocalVault(path, vf.vaultId, dek, keyProvider, vf.kek, secrets, keepVersions);
        }
        try {
            if (path.getParent() != null) {
                Files.createDirectories(path.getParent());
            }
        } catch (IOException e) {
            throw new CredentialException("create vault dir: " + e.getMessage(), e);
        }
        String vaultId = UUID.randomUUID().toString();
        byte[] dek = VaultCrypto.random(VaultCrypto.KEY_LEN);
        KekInfo kek = keyProvider.wrapDek(vaultId, dek);
        LocalVault v = new LocalVault(path, vaultId, dek, keyProvider, kek, new java.util.TreeMap<>(), keepVersions);
        v.save();
        return v;
    }

    public String vaultId() {
        return vaultId;
    }

    public Secret get(String name) {
        SecretEntry e = secrets.get(name);
        if (e == null || e.versions.isEmpty()) {
            return null;
        }
        return decrypt(name, e.versions.get(e.versions.size() - 1));
    }

    public Secret getVersion(String name, String version) {
        SecretEntry e = secrets.get(name);
        if (e == null) {
            return null;
        }
        for (VersionEntry v : e.versions) {
            if (v.version.equals(version)) {
                return decrypt(name, v);
            }
        }
        return null;
    }

    public boolean exists(String name) {
        SecretEntry e = secrets.get(name);
        return e != null && !e.versions.isEmpty();
    }

    public List<SecretMeta> list(String prefix) {
        List<String> names = new ArrayList<>(secrets.keySet());
        names.sort((a, b) -> Arrays.compareUnsigned(a.getBytes(StandardCharsets.UTF_8), b.getBytes(StandardCharsets.UTF_8)));
        List<SecretMeta> out = new ArrayList<>();
        for (String name : names) {
            if (!name.startsWith(prefix)) {
                continue;
            }
            List<VersionEntry> vs = secrets.get(name).versions;
            if (!vs.isEmpty()) {
                out.add(metaOf(name, vs.get(vs.size() - 1)));
            }
        }
        return out;
    }

    public List<String> versions(String name) {
        SecretEntry e = secrets.get(name);
        List<String> out = new ArrayList<>();
        if (e != null) {
            for (VersionEntry v : e.versions) {
                out.add(v.version);
            }
        }
        return out;
    }

    /** Upstream version id of the latest version of {@code name} (for sync change detection). */
    public String latestCentralVersionId(String name) {
        SecretEntry e = secrets.get(name);
        if (e != null && !e.versions.isEmpty()) {
            return e.versions.get(e.versions.size() - 1).centralVersionId;
        }
        return null;
    }

    public String put(String name, byte[] plaintext, PutOptions opts) {
        if (opts == null) {
            opts = PutOptions.defaults();
        }
        String version = nextVersion(name);
        byte[] nonce = VaultCrypto.random(VaultCrypto.NONCE_LEN);
        byte[] ct = VaultCrypto.seal(dek, nonce, VaultFormat.recordAad(vaultId, name, version), plaintext);
        VersionEntry rec = new VersionEntry();
        rec.version = version;
        rec.createdMs = System.currentTimeMillis();
        rec.source = opts.source != null ? opts.source : "local";
        rec.contentType = opts.contentType != null ? opts.contentType : "application/octet-stream";
        rec.ttlSecs = opts.ttlSecs;
        rec.centralVersionId = opts.centralVersionId;
        rec.labels = (opts.labels != null && !opts.labels.isEmpty()) ? opts.labels : null;
        rec.nonce = B64E.encodeToString(nonce);
        rec.ciphertext = B64E.encodeToString(ct);

        SecretEntry e = secrets.computeIfAbsent(name, k -> {
            SecretEntry se = new SecretEntry();
            se.versions = new ArrayList<>();
            return se;
        });
        e.versions.add(rec);
        if (e.versions.size() > keep) {
            e.versions.subList(0, e.versions.size() - keep).clear();
        }
        save();
        return version;
    }

    public boolean delete(String name) {
        if (secrets.remove(name) != null) {
            save();
            return true;
        }
        return false;
    }

    /** Re-read the vault if the file changed since last load (cross-process freshness). */
    public boolean reloadIfChanged() {
        String cur = fileStamp();
        if (java.util.Objects.equals(cur, stamp)) {
            return false;
        }
        VaultFile vf = readFile(path);
        verifyMac(dek, vf);
        this.secrets = vf.secrets != null ? vf.secrets : new java.util.TreeMap<>();
        this.kek = vf.kek;
        this.stamp = cur;
        return true;
    }

    private String nextVersion(String name) {
        long n = 0;
        SecretEntry e = secrets.get(name);
        if (e != null && !e.versions.isEmpty()) {
            try {
                n = Long.parseLong(e.versions.get(e.versions.size() - 1).version);
            } catch (NumberFormatException ignored) {
                n = 0;
            }
        }
        return String.format("%08d", n + 1);
    }

    private Secret decrypt(String name, VersionEntry v) {
        byte[] nonce = B64D.decode(v.nonce);
        byte[] ct = B64D.decode(v.ciphertext);
        byte[] pt = VaultCrypto.open(dek, nonce, VaultFormat.recordAad(vaultId, name, v.version), ct);
        return new Secret(name, v.version, pt, v.labels, v.createdMs,
                v.source != null ? v.source : "local",
                v.contentType != null ? v.contentType : "application/octet-stream");
    }

    private void save() {
        byte[] macKey = VaultCrypto.deriveMacKey(dek, vaultId.getBytes(StandardCharsets.UTF_8));
        String mac = B64E.encodeToString(VaultCrypto.hmac(macKey, VaultFormat.macInput(vaultId, secrets)));
        VaultFile vf = new VaultFile();
        vf.format = VaultFormat.FORMAT_VERSION;
        vf.vaultId = vaultId;
        vf.kek = kek;
        vf.secrets = secrets;
        vf.mac = mac;
        byte[] data = GSON.toJson(vf).getBytes(StandardCharsets.UTF_8);

        Path lockPath = path.resolveSibling(path.getFileName() + ".lock");
        Path tmp = path.resolveSibling(path.getFileName() + ".tmp");
        try (FileChannel ch = FileChannel.open(lockPath, StandardOpenOption.CREATE, StandardOpenOption.WRITE);
             FileLock ignored = ch.lock()) {
            Files.write(tmp, data);
            try {
                Files.move(tmp, path, StandardCopyOption.ATOMIC_MOVE);
            } catch (IOException atomicFail) {
                Files.move(tmp, path, StandardCopyOption.REPLACE_EXISTING);
            }
        } catch (IOException e) {
            throw new CredentialException("persist vault: " + e.getMessage(), e);
        }
        this.stamp = fileStamp();
    }

    private static VaultFile readFile(Path path) {
        try {
            String json = Files.readString(path, StandardCharsets.UTF_8);
            VaultFile vf = GSON.fromJson(json, VaultFile.class);
            if (vf == null) {
                throw new CredentialException("empty vault file");
            }
            return vf;
        } catch (IOException e) {
            throw new CredentialException("read vault: " + e.getMessage(), e);
        } catch (com.google.gson.JsonParseException e) {
            throw new CredentialException("parse vault: " + e.getMessage(), e);
        }
    }

    private static void verifyMac(byte[] dek, VaultFile vf) {
        byte[] macKey = VaultCrypto.deriveMacKey(dek, vf.vaultId.getBytes(StandardCharsets.UTF_8));
        byte[] expected = B64D.decode(vf.mac);
        java.util.TreeMap<String, SecretEntry> secrets = vf.secrets != null ? vf.secrets : new java.util.TreeMap<>();
        if (!VaultCrypto.hmacVerify(macKey, VaultFormat.macInput(vf.vaultId, secrets), expected)) {
            throw new CredentialException("vault integrity check failed (tampered or wrong key)");
        }
    }

    private static SecretMeta metaOf(String name, VersionEntry v) {
        return new SecretMeta(name, v.version, v.createdMs, v.ttlSecs,
                v.source != null ? v.source : "local",
                v.labels != null ? v.labels : java.util.Map.of());
    }

    private String fileStamp() {
        try {
            return Files.getLastModifiedTime(path).toMillis() + ":" + Files.size(path);
        } catch (IOException e) {
            return null;
        }
    }
}
