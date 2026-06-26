package com.mbreissi.ggcommons.credentials;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assumptions.assumeTrue;

import java.nio.charset.StandardCharsets;
import java.nio.file.Path;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import com.google.gson.JsonObject;

/**
 * PKCS#11 round-trip integration test against a real token (e.g. SoftHSM2). Gated by
 * {@code GGCOMMONS_IT_PKCS11=1}; skipped otherwise. Requires these env vars:
 * {@code PKCS11_MODULE}, {@code PKCS11_TOKEN}, {@code PKCS11_KEY}, {@code PKCS11_PIN} (and, for
 * SoftHSM, {@code SOFTHSM2_CONF}).
 */
class Pkcs11KeyProviderTest {

    @Test
    void pkcs11BackedVaultRoundTrip(@TempDir Path dir) {
        assumeTrue("1".equals(System.getenv("GGCOMMONS_IT_PKCS11")),
                "needs a PKCS#11 token (GGCOMMONS_IT_PKCS11=1)");
        String module = System.getenv("PKCS11_MODULE");
        String token = System.getenv("PKCS11_TOKEN");
        String key = System.getenv("PKCS11_KEY");
        String pin = System.getenv("PKCS11_PIN");

        JsonObject cfg = config(dir, module, token, key, pin);
        // Open a pkcs11-backed vault (DEK wrapped by the HSM key), put a secret, read it back.
        CredentialService creds = Credentials.open(cfg);
        creds.put("db/password", "s3cr3t".getBytes(StandardCharsets.UTF_8));
        assertEquals("s3cr3t", creds.getString("db/password").orElseThrow());

        // Reopen the persisted vault: forces a fresh HSM unwrap of the DEK (fail-closed otherwise).
        CredentialService reopened = Credentials.open(config(dir, module, token, key, pin));
        assertEquals("s3cr3t", reopened.getString("db/password").orElseThrow());
    }

    private static JsonObject config(Path dir, String module, String token, String key, String pin) {
        JsonObject kp = new JsonObject();
        kp.addProperty("type", "pkcs11");
        kp.addProperty("modulePath", module);
        kp.addProperty("tokenLabel", token);
        kp.addProperty("keyLabel", key);
        kp.addProperty("pin", pin);

        JsonObject vault = new JsonObject();
        vault.addProperty("path", dir.resolve("vault").toString());
        vault.add("keyProvider", kp);

        JsonObject cfg = new JsonObject();
        cfg.add("vault", vault);
        return cfg;
    }
}
