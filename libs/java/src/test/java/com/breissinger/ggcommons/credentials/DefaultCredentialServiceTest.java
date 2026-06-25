package com.breissinger.ggcommons.credentials;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import java.util.Optional;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/**
 * Unit tests for {@link DefaultCredentialService} focused on the parts not already exercised by
 * {@code VaultTest}/{@code CredentialAuditTest}: typed-view missing-field errors, list/versions
 * namespacing round-trips, {@link PutOptions} passthrough (labels/source/contentType), and the
 * {@code stats()}/{@code refresh()} delegation to a real {@link SyncEngine} (driven by an in-memory
 * central source, no AWS).
 */
class DefaultCredentialServiceTest {

    private static DefaultCredentialService svc(Path dir) {
        return svc(dir, "");
    }

    private static DefaultCredentialService svc(Path dir, String namespace) {
        LocalVault v = LocalVault.open(dir.resolve("vault"), new FileKeyProvider(new byte[32]), 5);
        return new DefaultCredentialService(v, namespace, new Object(), null);
    }

    private static byte[] utf8(String s) {
        return s.getBytes(StandardCharsets.UTF_8);
    }

    // ---- typed-view missing-field error paths (Optional present, but wrong shape) ----

    @Test
    void awsCredentialsMissingFieldsThrows(@TempDir Path dir) {
        DefaultCredentialService c = svc(dir);
        c.put("aws", utf8("{\"accessKeyId\":\"AKIA\"}")); // no secretAccessKey
        CredentialException ex = assertThrows(CredentialException.class, () -> c.getAwsCredentials("aws"));
        assertTrue(ex.getMessage().contains("AWS credentials"));
    }

    @Test
    void basicAuthMissingPasswordThrows(@TempDir Path dir) {
        DefaultCredentialService c = svc(dir);
        c.put("ba", utf8("{\"username\":\"u\"}"));
        assertThrows(CredentialException.class, () -> c.getBasicAuth("ba"));
    }

    @Test
    void tlsBundleMissingKeyThrows(@TempDir Path dir) {
        DefaultCredentialService c = svc(dir);
        c.put("tls", utf8("{\"certPem\":\"C\"}"));
        assertThrows(CredentialException.class, () -> c.getTlsBundle("tls"));
    }

    @Test
    void kafkaSaslMissingUserThrowsAndMechanismDefaultsToPlain(@TempDir Path dir) {
        DefaultCredentialService c = svc(dir);
        c.put("bad", utf8("{\"password\":\"p\"}"));
        assertThrows(CredentialException.class, () -> c.getKafkaSasl("bad"));

        c.put("ok", utf8("{\"username\":\"u\",\"password\":\"p\"}"));
        KafkaSasl k = c.getKafkaSasl("ok").orElseThrow();
        assertEquals("PLAIN", k.mechanism());

        c.put("scram", utf8("{\"mechanism\":\"SCRAM-SHA-512\",\"username\":\"u\",\"password\":\"p\"}"));
        assertEquals("SCRAM-SHA-512", c.getKafkaSasl("scram").orElseThrow().mechanism());
    }

    @Test
    void typedViewOnMissingSecretIsEmptyNotError(@TempDir Path dir) {
        DefaultCredentialService c = svc(dir);
        assertTrue(c.getAwsCredentials("absent").isEmpty());
        assertTrue(c.getBasicAuth("absent").isEmpty());
        assertTrue(c.getTlsBundle("absent").isEmpty());
        assertTrue(c.getKafkaSasl("absent").isEmpty());
    }

    @Test
    void invalidJsonForTypedViewThrows(@TempDir Path dir) {
        DefaultCredentialService c = svc(dir);
        c.put("garbage", utf8("not-json{"));
        assertThrows(CredentialException.class, () -> c.getAwsCredentials("garbage"));
    }

    // ---- put options + metadata passthrough ----

    @Test
    void putOptionsLabelsSourceContentTypeRoundTrip(@TempDir Path dir) {
        DefaultCredentialService c = svc(dir);
        PutOptions opts = new PutOptions()
                .labels(Map.of("env", "prod"))
                .contentType("application/json");
        opts.source = "central";
        c.put("k", utf8("{\"a\":1}"), opts);

        Secret s = c.get("k").orElseThrow();
        assertEquals("central", s.source());
        assertEquals("application/json", s.contentType());
        assertEquals("prod", s.labels().get("env"));

        SecretMeta meta = c.list("").get(0);
        assertEquals("k", meta.name());
        assertEquals("central", meta.source());
        assertEquals("prod", meta.labels().get("env"));
    }

    @Test
    void existsReflectsPutAndDelete(@TempDir Path dir) {
        DefaultCredentialService c = svc(dir);
        assertFalse(c.exists("k"));
        c.put("k", utf8("v"));
        assertTrue(c.exists("k"));
        assertTrue(c.delete("k"));
        assertFalse(c.exists("k"));
    }

    // ---- namespacing round-trips through list/versions/get ----

    @Test
    void listAndVersionsStripNamespace(@TempDir Path dir) {
        DefaultCredentialService c = svc(dir, "thing/Comp");
        c.put("a/one", utf8("1"));
        c.put("a/two", utf8("2"));
        c.put("a/two", utf8("2b"));

        List<String> names = c.list("").stream().map(SecretMeta::name).sorted().toList();
        assertEquals(List.of("a/one", "a/two"), names);

        // prefix is relative -> namespaced internally
        assertEquals(List.of("a/two"), c.list("a/two").stream().map(SecretMeta::name).toList());

        // versions returned for the relative name
        assertEquals(2, c.versions("a/two").size());
        assertEquals("a/two", c.get("a/two").orElseThrow().name());
        assertEquals("2b", c.get("a/two").orElseThrow().asString());
    }

    @Test
    void getVersionStripsNamespace(@TempDir Path dir) {
        DefaultCredentialService c = svc(dir, "ns");
        String ver = c.put("k", utf8("v1"));
        assertEquals("k", c.getVersion("k", ver).orElseThrow().name());
        assertTrue(c.getVersion("k", "nope").isEmpty());
    }

    // ---- stats() / refresh() with no sync engine ----

    @Test
    void statsWithoutSyncReportsCountOnly(@TempDir Path dir) {
        DefaultCredentialService c = svc(dir);
        c.put("a", utf8("1"));
        c.put("b", utf8("2"));
        CredentialStats s = c.stats();
        assertEquals(2, s.secretCount());
        assertNull(s.lastSyncAgeMs());
        assertEquals(0, s.syncFailures());
        assertEquals(0, s.rotations());
        // refresh is a no-op without a sync engine (must not throw)
        c.refresh();
    }

    // ---- stats() / refresh() delegate to a real SyncEngine ----

    @Test
    void statsAndRefreshDelegateToSyncEngine(@TempDir Path dir) {
        LocalVault v = LocalVault.open(dir.resolve("vault"), new FileKeyProvider(new byte[32]), 5);
        Object lock = new Object();

        // central source returns one rotating secret under the namespaced key.
        final String[] value = {"v1"};
        final String[] version = {"c1"};
        CentralVaultSource src = name -> Optional.of(new CentralSecret(
                value[0].getBytes(StandardCharsets.UTF_8), version[0], Map.of()));

        SyncEngine sync = new SyncEngine(v, lock, src, "ns", List.of(
                new SyncEngine.SyncSecret("k", null)), 0, true); // bootstrap seeds k
        try {
            DefaultCredentialService c = new DefaultCredentialService(v, "ns", lock, sync);

            CredentialStats s1 = c.stats();
            assertEquals(1, s1.secretCount());
            assertNotNull(s1.lastSyncAgeMs());          // sync engine ran a successful pass
            assertTrue(s1.lastSyncAgeMs() >= 0);
            assertEquals(0, s1.syncFailures());
            assertEquals(1, s1.rotations());

            // rotate upstream + force a refresh through the service -> delegates to syncNow()
            value[0] = "v2";
            version[0] = "c2";
            c.refresh();

            assertEquals("v2", c.get("k").orElseThrow().asString());
            assertEquals(2, c.stats().rotations());
        } finally {
            sync.close();
        }
    }

    @Test
    void crossProcessChangeIsPickedUpOnRead(@TempDir Path dir) {
        // Two services over the same file; a write by one is visible to the other via reloadIfChanged.
        Path path = dir.resolve("vault");
        DefaultCredentialService writer = new DefaultCredentialService(
                LocalVault.open(path, new FileKeyProvider(new byte[32]), 5), "", new Object(), null);
        DefaultCredentialService reader = new DefaultCredentialService(
                LocalVault.open(path, new FileKeyProvider(new byte[32]), 5), "", new Object(), null);

        assertTrue(reader.get("shared").isEmpty());
        writer.put("shared", utf8("hi"));
        Optional<Secret> got = reader.get("shared");
        assertTrue(got.isPresent());
        assertEquals("hi", got.get().asString());
    }
}
