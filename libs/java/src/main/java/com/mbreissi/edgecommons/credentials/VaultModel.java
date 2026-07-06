package com.mbreissi.edgecommons.credentials;

import java.util.List;
import java.util.Map;
import java.util.TreeMap;

/**
 * Gson POJOs for the on-disk vault file (camelCase JSON). Null fields are omitted by Gson,
 * matching the Rust/Python {@code skip_serializing_if} behavior.
 */
public final class VaultModel {
    private VaultModel() {}

    /** The whole vault file. */
    public static final class VaultFile {
        public int format;
        public String vaultId;
        public KekInfo kek;
        /** TreeMap → JSON keys sorted (cosmetic; the MAC is over canonical bytes). */
        public TreeMap<String, SecretEntry> secrets = new TreeMap<>();
        public String mac;
    }

    /** How the DEK is wrapped. */
    public static final class KekInfo {
        public String provider;
        public String alg;
        public String wrapNonce;
        public String wrappedDek;
        public String kmsKeyId;
    }

    /** All retained versions of one secret (newest last). */
    public static final class SecretEntry {
        public List<VersionEntry> versions;
    }

    /** One encrypted version of a secret. */
    public static final class VersionEntry {
        public String version;
        public long createdMs;
        public Long ttlSecs;
        public String source;
        public String centralVersionId;
        public Map<String, String> labels;
        public String contentType;
        public String nonce;
        public String ciphertext;
    }
}
