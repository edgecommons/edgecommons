package com.aws.proserve.ggcommons.credentials;

import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Base64;
import java.util.List;
import java.util.Map;

import com.aws.proserve.ggcommons.credentials.VaultModel.SecretEntry;
import com.aws.proserve.ggcommons.credentials.VaultModel.VersionEntry;

/**
 * On-disk format byte constructions (normative; must match the Rust/Python references).
 *
 * <p>The AEAD AADs and the length-prefixed canonical MAC input are defined here. The MAC is taken
 * over this byte string (not the JSON text), so JSON formatting may differ across languages while
 * the integrity check stays identical. See {@code docs/CREDENTIALS.md} §4.
 */
public final class VaultFormat {
    public static final int FORMAT_VERSION = 1;
    private static final Base64.Decoder B64D = Base64.getDecoder();

    private VaultFormat() {}

    /** AEAD AAD binding a record to its vault, name, and version. */
    public static byte[] recordAad(String vaultId, String name, String version) {
        return ("ggcommons-vault/v1|" + vaultId + "|" + name + "|" + version).getBytes(StandardCharsets.UTF_8);
    }

    /** AEAD AAD binding the wrapped DEK to its vault. */
    public static byte[] dekWrapAad(String vaultId) {
        return ("ggcommons-vault/v1/dek-wrap|" + vaultId).getBytes(StandardCharsets.UTF_8);
    }

    /**
     * Build the canonical MAC input over the whole secret set. Secrets are ordered by their UTF-8
     * name bytes (unsigned, matching Rust's {@code BTreeMap} / Python's sort) — not Java String
     * order, which differs for non-ASCII names. Layout: see the Rust reference {@code mac_input}.
     */
    public static byte[] macInput(String vaultId, Map<String, SecretEntry> secrets) {
        ByteArrayOutputStream out = new ByteArrayOutputStream(256);
        writeBytes(out, "ggcommons-vault/v1/mac".getBytes(StandardCharsets.UTF_8));
        lp(out, vaultId.getBytes(StandardCharsets.UTF_8));
        u32le(out, secrets.size());

        List<String> names = new ArrayList<>(secrets.keySet());
        names.sort((a, b) -> Arrays.compareUnsigned(a.getBytes(StandardCharsets.UTF_8), b.getBytes(StandardCharsets.UTF_8)));
        for (String name : names) {
            lp(out, name.getBytes(StandardCharsets.UTF_8));
            List<VersionEntry> versions = secrets.get(name).versions;
            u32le(out, versions.size());
            for (VersionEntry v : versions) {
                lp(out, v.version.getBytes(StandardCharsets.UTF_8));
                u64le(out, v.createdMs);
                u64le(out, v.ttlSecs == null ? 0L : v.ttlSecs);
                lp(out, (v.source == null ? "" : v.source).getBytes(StandardCharsets.UTF_8));
                lp(out, (v.centralVersionId == null ? "" : v.centralVersionId).getBytes(StandardCharsets.UTF_8));
                lp(out, B64D.decode(v.nonce));
                lp(out, B64D.decode(v.ciphertext));
            }
        }
        return out.toByteArray();
    }

    private static void lp(ByteArrayOutputStream out, byte[] b) {
        u32le(out, b.length);
        writeBytes(out, b);
    }

    private static void u32le(ByteArrayOutputStream out, int n) {
        out.write(n & 0xFF);
        out.write((n >>> 8) & 0xFF);
        out.write((n >>> 16) & 0xFF);
        out.write((n >>> 24) & 0xFF);
    }

    private static void u64le(ByteArrayOutputStream out, long n) {
        for (int i = 0; i < 8; i++) {
            out.write((int) ((n >>> (8 * i)) & 0xFF));
        }
    }

    private static void writeBytes(ByteArrayOutputStream out, byte[] b) {
        out.write(b, 0, b.length);
    }
}
