package com.breissinger.ggcommons.credentials;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;

/**
 * Unit tests for the {@link Credentials} factory: building a {@link CredentialService} and
 * {@link KeyProvider} from the {@code credentials} config section without touching live AWS/HSM.
 * Covers the default + explicit file key provider, the {@code kms}/{@code pkcs11} validation error
 * paths, unsupported key-provider / central types, the {@code none} central source, and the
 * {@code awsSecretsManager} wiring (with bootstrap off so no network call is made).
 */
class CredentialsFactoryTest {

    private static JsonObject vaultCfg(Path dir, JsonObject keyProvider) {
        JsonObject vault = new JsonObject();
        vault.addProperty("path", dir.resolve("vault").toString());
        vault.addProperty("keepVersions", 3);
        if (keyProvider != null) {
            vault.add("keyProvider", keyProvider);
        }
        JsonObject cfg = new JsonObject();
        cfg.add("vault", vault);
        return cfg;
    }

    // ---- default config (no keyProvider, no central) ----

    @Test
    void defaultConfigOpensWorkingFileBackedVault(@TempDir Path dir) {
        CredentialService c = Credentials.open(vaultCfg(dir, null));
        c.put("k", "v".getBytes(StandardCharsets.UTF_8));
        assertEquals("v", c.getString("k").orElseThrow());
        // file key provider auto-generated the default <vaultPath>.key
        assertTrue(Files.exists(dir.resolve("vault.key")));
    }

    @Test
    void nullConfigUsesDefaultsRelativeVault() {
        // open(null) must not NPE; it builds a default ("vault") relative store.
        CredentialService c = Credentials.open(null);
        assertNotNull(c);
    }

    @Test
    void namespaceIsAppliedThroughTheFactory(@TempDir Path dir) throws Exception {
        CredentialService c = Credentials.open(vaultCfg(dir, null), "thing/Comp");
        c.put("k", "v".getBytes(StandardCharsets.UTF_8));
        assertEquals("v", c.getString("k").orElseThrow());
        String raw = Files.readString(dir.resolve("vault"));
        assertTrue(raw.contains("thing/Comp/k"));
    }

    // ---- file key provider with explicit keyPath ----

    @Test
    void fileKeyProviderHonoursExplicitKeyPath(@TempDir Path dir) {
        JsonObject kp = new JsonObject();
        kp.addProperty("type", "file");
        Path keyPath = dir.resolve("custom/my.key");
        kp.addProperty("keyPath", keyPath.toString());

        CredentialService c = Credentials.open(vaultCfg(dir, kp));
        c.put("k", "v".getBytes(StandardCharsets.UTF_8));
        assertTrue(Files.exists(keyPath), "explicit key path should be created");
    }

    @Test
    void buildKeyProviderDefaultsToFile(@TempDir Path dir) {
        KeyProvider p = Credentials.buildKeyProvider(new JsonObject(), dir.resolve("k.key").toString());
        assertEquals("file", p.providerId());
        assertTrue(Files.exists(dir.resolve("k.key")));
    }

    @Test
    void buildKeyProviderReusesExistingKeyFile(@TempDir Path dir) throws Exception {
        Path keyFile = dir.resolve("k.key");
        Files.write(keyFile, new byte[32]); // pre-existing valid 32-byte key
        KeyProvider p = Credentials.buildKeyProvider(new JsonObject(), keyFile.toString());
        assertEquals("file", p.providerId());
        // unchanged on disk (loaded, not regenerated)
        assertEquals(32, Files.size(keyFile));
    }

    // ---- kms key provider validation (no live KMS) ----

    @Test
    void kmsKeyProviderRequiresKmsKeyId(@TempDir Path dir) {
        JsonObject kp = new JsonObject();
        kp.addProperty("type", "kms");
        CredentialException ex = assertThrows(CredentialException.class,
                () -> Credentials.buildKeyProvider(kp, dir.resolve("k.key").toString()));
        assertTrue(ex.getMessage().contains("kmsKeyId"));
    }

    @Test
    void greengrassKeyProviderRequiresKmsKeyId(@TempDir Path dir) {
        JsonObject kp = new JsonObject();
        kp.addProperty("type", "greengrass");
        assertThrows(CredentialException.class,
                () -> Credentials.buildKeyProvider(kp, dir.resolve("k.key").toString()));
    }

    @Test
    void kmsKeyProviderBuildsWhenKeyIdPresent(@TempDir Path dir) {
        // Construction must not call KMS (lazy SDK client) -> succeeds offline.
        JsonObject kp = new JsonObject();
        kp.addProperty("type", "kms");
        kp.addProperty("kmsKeyId", "alias/test");
        kp.addProperty("region", "us-east-1");
        kp.addProperty("endpointUrl", "http://localhost:4566");
        KeyProvider p = Credentials.buildKeyProvider(kp, dir.resolve("k.key").toString());
        assertNotNull(p);
    }

    // ---- pkcs11 key provider validation (no live HSM) ----

    @Test
    void pkcs11RequiresModulePath(@TempDir Path dir) {
        JsonObject kp = new JsonObject();
        kp.addProperty("type", "pkcs11");
        CredentialException ex = assertThrows(CredentialException.class,
                () -> Credentials.buildKeyProvider(kp, dir.resolve("k.key").toString()));
        assertTrue(ex.getMessage().contains("modulePath"));
    }

    @Test
    void pkcs11RequiresKeyLabel(@TempDir Path dir) {
        JsonObject kp = new JsonObject();
        kp.addProperty("type", "pkcs11");
        kp.addProperty("modulePath", "/opt/lib/softhsm.so");
        CredentialException ex = assertThrows(CredentialException.class,
                () -> Credentials.buildKeyProvider(kp, dir.resolve("k.key").toString()));
        assertTrue(ex.getMessage().contains("keyLabel"));
    }

    @Test
    void pkcs11RequiresPinOrPinEnv(@TempDir Path dir) {
        JsonObject kp = new JsonObject();
        kp.addProperty("type", "pkcs11");
        kp.addProperty("modulePath", "/opt/lib/softhsm.so");
        kp.addProperty("keyLabel", "ggcommons-kek");
        CredentialException ex = assertThrows(CredentialException.class,
                () -> Credentials.buildKeyProvider(kp, dir.resolve("k.key").toString()));
        assertTrue(ex.getMessage().contains("pinEnv") || ex.getMessage().contains("pin"));
    }

    @Test
    void pkcs11PinEnvUnsetThrows(@TempDir Path dir) {
        JsonObject kp = new JsonObject();
        kp.addProperty("type", "pkcs11");
        kp.addProperty("modulePath", "/opt/lib/softhsm.so");
        kp.addProperty("keyLabel", "ggcommons-kek");
        kp.addProperty("pinEnv", "GGCOMMONS_NONEXISTENT_PKCS11_PIN_ENV");
        CredentialException ex = assertThrows(CredentialException.class,
                () -> Credentials.buildKeyProvider(kp, dir.resolve("k.key").toString()));
        assertTrue(ex.getMessage().contains("is not set"));
    }

    // ---- unsupported types ----

    @Test
    void unsupportedKeyProviderTypeThrows(@TempDir Path dir) {
        JsonObject kp = new JsonObject();
        kp.addProperty("type", "magic");
        CredentialException ex = assertThrows(CredentialException.class,
                () -> Credentials.buildKeyProvider(kp, dir.resolve("k.key").toString()));
        assertTrue(ex.getMessage().contains("magic"));
    }

    @Test
    void unsupportedCentralTypeThrows(@TempDir Path dir) {
        JsonObject cfg = vaultCfg(dir, null);
        JsonObject central = new JsonObject();
        central.addProperty("type", "vault-server");
        cfg.add("central", central);
        CredentialException ex = assertThrows(CredentialException.class, () -> Credentials.open(cfg));
        assertTrue(ex.getMessage().contains("vault-server"));
    }

    // ---- central none vs awsSecretsManager ----

    @Test
    void centralNoneGivesPlainServiceWithoutSync(@TempDir Path dir) {
        JsonObject cfg = vaultCfg(dir, null);
        JsonObject central = new JsonObject();
        central.addProperty("type", "none");
        cfg.add("central", central);
        CredentialService c = Credentials.open(cfg);
        c.put("k", "v".getBytes(StandardCharsets.UTF_8));
        // no sync engine -> lastSyncAgeMs null, refresh no-op
        assertTrue(c.stats().lastSyncAgeMs() == null);
        c.refresh();
        assertEquals(1, c.stats().secretCount());
    }

    @Test
    void awsSecretsManagerWiresSyncEngineWithoutNetwork(@TempDir Path dir) {
        // bootstrapOnStart=false + refreshIntervalSecs=0 -> SyncEngine never calls the SDK,
        // so the factory wires the central source offline (no AWS).
        JsonObject central = new JsonObject();
        central.addProperty("type", "awsSecretsManager");
        central.addProperty("region", "us-east-1");
        central.addProperty("endpointUrl", "http://localhost:4566");
        central.addProperty("bootstrapOnStart", false);
        central.addProperty("refreshIntervalSecs", 0);

        JsonArray secrets = new JsonArray();
        secrets.add("plain/name");
        JsonObject objEntry = new JsonObject();
        objEntry.addProperty("name", "db/password");
        objEntry.addProperty("from", "shared/db");
        secrets.add(objEntry);
        JsonObject sync = new JsonObject();
        sync.add("secrets", secrets);
        central.add("sync", sync);

        JsonObject cfg = vaultCfg(dir, null);
        cfg.add("central", central);

        CredentialService c = Credentials.open(cfg, "ns");
        assertNotNull(c);
        // local ops still work and stats reflect a wired (but never-run) sync engine.
        c.put("local", "v".getBytes(StandardCharsets.UTF_8));
        assertEquals(1, c.stats().secretCount());
        assertTrue(c.stats().lastSyncAgeMs() == null); // bootstrap off, never synced
    }

    @Test
    void auditDisabledByConfigStillOpens(@TempDir Path dir) {
        JsonObject cfg = vaultCfg(dir, null);
        JsonObject audit = new JsonObject();
        audit.addProperty("enabled", false);
        cfg.add("audit", audit);
        CredentialService c = Credentials.open(cfg);
        c.put("k", "v".getBytes(StandardCharsets.UTF_8));
        assertTrue(c.exists("k"));
    }
}
