package com.mbreissi.edgecommons.credentials;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import com.google.gson.Gson;
import com.google.gson.JsonObject;

/**
 * Unit tests for {@link LocalVault} (the encrypted local store) exercised directly against a
 * {@link FileKeyProvider} — covers get/getVersion/exists/list/versions, put with options, delete,
 * version pruning, persistence/reopen, reload-on-change, central-version tracking, and the
 * unsupported-format error. Crypto/format byte-parity is pinned in {@link VaultTest}.
 */
class LocalVaultTest {

    private static LocalVault open(Path dir, int keep) {
        return LocalVault.open(dir.resolve("vault"), new FileKeyProvider(new byte[32]), keep);
    }

    @Test
    void newVaultHasIdAndEmptyState(@TempDir Path dir) {
        LocalVault v = open(dir, 3);
        assertNotNull(v.vaultId());
        assertFalse(v.vaultId().isBlank());
        assertTrue(v.list("").isEmpty());
        assertNull(v.get("nope"));
        assertFalse(v.exists("nope"));
        assertTrue(v.versions("nope").isEmpty());
        assertTrue(Files.exists(dir.resolve("vault")), "open() must persist a new empty vault");
    }

    @Test
    void putGetRoundTripAndMetadataDefaults(@TempDir Path dir) {
        LocalVault v = open(dir, 5);
        String version = v.put("db/pw", "s3cr3t".getBytes(StandardCharsets.UTF_8), null);
        assertEquals("00000001", version);

        Secret s = v.get("db/pw");
        assertNotNull(s);
        assertArrayEquals("s3cr3t".getBytes(StandardCharsets.UTF_8), s.bytes());
        assertEquals("db/pw", s.name());
        assertEquals("00000001", s.version());
        assertEquals("local", s.source());
        assertEquals("application/octet-stream", s.contentType());
        assertTrue(s.createdMs() > 0);
        assertTrue(v.exists("db/pw"));
    }

    @Test
    void putHonorsPutOptions(@TempDir Path dir) {
        LocalVault v = open(dir, 5);
        PutOptions opts = PutOptions.defaults()
                .ttlSecs(120)
                .contentType("application/json")
                .labels(Map.of("env", "prod"));
        opts.source = "central";
        opts.centralVersionId = "cv-42";
        v.put("svc", "{}".getBytes(StandardCharsets.UTF_8), opts);

        Secret s = v.get("svc");
        assertEquals("central", s.source());
        assertEquals("application/json", s.contentType());
        assertEquals(Map.of("env", "prod"), s.labels());

        SecretMeta meta = v.list("").get(0);
        assertEquals("svc", meta.name());
        assertEquals(Long.valueOf(120), meta.ttlSecs());
        assertEquals("central", meta.source());
        assertEquals("cv-42", v.latestCentralVersionId("svc"));
    }

    @Test
    void emptyLabelsStoredAsNullAndReturnedNull(@TempDir Path dir) {
        LocalVault v = open(dir, 5);
        PutOptions opts = PutOptions.defaults().labels(Map.of());
        v.put("k", "v".getBytes(), opts);
        assertNull(v.get("k").labels(), "empty labels map is normalized to null on disk");
    }

    @Test
    void versionsAreMonotonicAndPrunedToKeep(@TempDir Path dir) {
        LocalVault v = open(dir, 2);
        v.put("k", "v1".getBytes(), null);
        v.put("k", "v2".getBytes(), null);
        v.put("k", "v3".getBytes(), null);
        assertEquals(List.of("00000002", "00000003"), v.versions("k"));
        assertEquals("v3", v.get("k").asString());
        assertEquals("v2", v.getVersion("k", "00000002").asString());
        assertNull(v.getVersion("k", "00000001"), "pruned version must be gone");
        assertNull(v.getVersion("k", "99999999"), "unknown version returns null");
        assertNull(v.getVersion("missing", "00000001"), "unknown name returns null");
    }

    @Test
    void keepIsClampedToAtLeastOne(@TempDir Path dir) {
        LocalVault v = open(dir, 0); // clamped to 1
        v.put("k", "v1".getBytes(), null);
        v.put("k", "v2".getBytes(), null);
        assertEquals(List.of("00000002"), v.versions("k"));
    }

    @Test
    void listFiltersByPrefixAndSortsByName(@TempDir Path dir) {
        LocalVault v = open(dir, 3);
        v.put("svc/b", "1".getBytes(), null);
        v.put("db/a", "2".getBytes(), null);
        v.put("svc/a", "3".getBytes(), null);

        assertEquals(List.of("db/a", "svc/a", "svc/b"),
                v.list("").stream().map(SecretMeta::name).toList());
        assertEquals(List.of("svc/a", "svc/b"),
                v.list("svc/").stream().map(SecretMeta::name).toList());
        assertTrue(v.list("nomatch").isEmpty());
    }

    @Test
    void deleteRemovesSecretAndReportsResult(@TempDir Path dir) {
        LocalVault v = open(dir, 3);
        v.put("k", "v".getBytes(), null);
        assertTrue(v.delete("k"));
        assertFalse(v.exists("k"));
        assertNull(v.get("k"));
        assertFalse(v.delete("k"), "second delete returns false");
        assertFalse(v.delete("never-existed"));
    }

    @Test
    void persistsAcrossReopen(@TempDir Path dir) {
        open(dir, 3).put("token", "abc".getBytes(StandardCharsets.UTF_8), null);
        LocalVault reopened = open(dir, 3);
        assertEquals("abc", reopened.get("token").asString());
    }

    @Test
    void vaultIdStableAcrossReopen(@TempDir Path dir) {
        String id = open(dir, 3).vaultId();
        assertEquals(id, open(dir, 3).vaultId());
    }

    @Test
    void reloadIfChangedPicksUpExternalWrites(@TempDir Path dir) {
        LocalVault writer = open(dir, 3);
        LocalVault reader = open(dir, 3);
        // reader has not seen the write yet
        assertFalse(reader.reloadIfChanged(), "no change since open → false");

        writer.put("k", "fresh".getBytes(StandardCharsets.UTF_8), null);
        assertTrue(reader.reloadIfChanged(), "file changed on disk → reload true");
        assertEquals("fresh", reader.get("k").asString());
        assertFalse(reader.reloadIfChanged(), "second call sees no further change");
    }

    @Test
    void wrongKekFailsClosedOnOpen(@TempDir Path dir) {
        open(dir, 3).put("k", "v".getBytes(), null);
        byte[] wrong = new byte[32];
        java.util.Arrays.fill(wrong, (byte) 7);
        assertThrows(CredentialException.class,
                () -> LocalVault.open(dir.resolve("vault"), new FileKeyProvider(wrong), 3));
    }

    @Test
    void unsupportedFormatVersionRejected(@TempDir Path dir) throws Exception {
        open(dir, 3).put("k", "v".getBytes(), null);
        Path path = dir.resolve("vault");
        Gson gson = new Gson();
        JsonObject vf = gson.fromJson(Files.readString(path), JsonObject.class);
        vf.addProperty("format", 99);
        Files.writeString(path, gson.toJson(vf));
        CredentialException ex = assertThrows(CredentialException.class,
                () -> LocalVault.open(path, new FileKeyProvider(new byte[32]), 3));
        assertTrue(ex.getMessage().contains("unsupported vault format"));
    }

    @Test
    void corruptJsonRejected(@TempDir Path dir) throws Exception {
        Path path = dir.resolve("vault");
        Files.writeString(path, "{ this is not valid json ");
        assertThrows(CredentialException.class,
                () -> LocalVault.open(path, new FileKeyProvider(new byte[32]), 3));
    }

    @Test
    void emptyVaultFileRejected(@TempDir Path dir) throws Exception {
        Path path = dir.resolve("vault");
        Files.writeString(path, "null");
        assertThrows(CredentialException.class,
                () -> LocalVault.open(path, new FileKeyProvider(new byte[32]), 3));
    }

    @Test
    void atomicWriteLeavesNoTempOrLockGarbageBlockingReopen(@TempDir Path dir) {
        LocalVault v = open(dir, 3);
        v.put("k", "v".getBytes(), null);
        // The .tmp file must have been renamed away; the canonical file is the only data file.
        assertFalse(Files.exists(dir.resolve("vault.tmp")), "temp file must be renamed, not left behind");
        // Reopen must still work (proves the rename produced a valid, MAC-verified file).
        assertEquals("v", open(dir, 3).get("k").asString());
    }
}
