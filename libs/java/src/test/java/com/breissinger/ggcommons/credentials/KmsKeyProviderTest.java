package com.breissinger.ggcommons.credentials;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assumptions.assumeTrue;

import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import com.google.gson.JsonObject;

import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.kms.KmsClient;
import software.amazon.awssdk.services.kms.model.CreateKeyResponse;

/**
 * KMS round-trip integration test against floci KMS at http://localhost:4566. Gated by
 * {@code GGCOMMONS_IT_KMS=1}; skipped otherwise.
 */
class KmsKeyProviderTest {

    private static final String ENDPOINT = "http://localhost:4566";

    @Test
    void kmsBackedVaultRoundTrip(@TempDir Path dir) {
        assumeTrue("1".equals(System.getenv("GGCOMMONS_IT_KMS")), "needs floci kms (GGCOMMONS_IT_KMS=1)");
        System.setProperty("aws.accessKeyId", "test");
        System.setProperty("aws.secretAccessKey", "test");
        System.setProperty("aws.region", "us-east-1");

        KmsClient kms = KmsClient.builder()
                .region(Region.US_EAST_1)
                .endpointOverride(URI.create(ENDPOINT))
                .build();
        CreateKeyResponse key = kms.createKey();
        String keyId = key.keyMetadata().keyId();

        JsonObject cfg = config(dir, keyId);
        // Open a kms-backed vault, put a secret, then reopen (forces a kms:Decrypt unwrap).
        CredentialService creds = Credentials.open(cfg);
        creds.put("db/password", "s3cr3t".getBytes(StandardCharsets.UTF_8));
        assertEquals("s3cr3t", creds.getString("db/password").orElseThrow());

        CredentialService reopened = Credentials.open(config(dir, keyId));
        assertEquals("s3cr3t", reopened.getString("db/password").orElseThrow());
    }

    private static JsonObject config(Path dir, String keyId) {
        JsonObject kp = new JsonObject();
        kp.addProperty("type", "kms");
        kp.addProperty("kmsKeyId", keyId);
        kp.addProperty("region", "us-east-1");
        kp.addProperty("endpointUrl", ENDPOINT);

        JsonObject vault = new JsonObject();
        vault.addProperty("path", dir.resolve("vault").toString());
        vault.add("keyProvider", kp);

        JsonObject cfg = new JsonObject();
        cfg.add("vault", vault);
        return cfg;
    }
}
