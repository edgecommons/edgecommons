package com.aws.proserve.ggcommons.credentials;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assumptions.assumeTrue;

import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.List;
import java.util.UUID;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;

import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.secretsmanager.SecretsManagerClient;
import software.amazon.awssdk.services.secretsmanager.model.CreateSecretRequest;
import software.amazon.awssdk.services.secretsmanager.model.DeleteSecretRequest;
import software.amazon.awssdk.services.secretsmanager.model.PutSecretValueRequest;

class CredentialSyncTest {

    @Test
    void namespacingIsolatesComponents(@TempDir Path dir) throws Exception {
        byte[] kek = new byte[32];
        Arrays.fill(kek, (byte) 5);
        Path path = dir.resolve("vault");
        CredentialService c1 = new DefaultCredentialService(
                LocalVault.open(path, new FileKeyProvider(kek), 2), "thing-1/CompA", new Object(), null);
        CredentialService c2 = new DefaultCredentialService(
                LocalVault.open(path, new FileKeyProvider(kek), 2), "thing-1/CompB", new Object(), null);

        c1.put("db/password", "a-secret".getBytes(StandardCharsets.UTF_8));
        c2.put("db/password", "b-secret".getBytes(StandardCharsets.UTF_8));

        assertEquals("a-secret", c1.getString("db/password").orElseThrow());
        assertEquals("b-secret", c2.getString("db/password").orElseThrow());
        assertEquals(List.of("db/password"), c1.list("").stream().map(SecretMeta::name).toList());

        String raw = Files.readString(path);
        assertTrue(raw.contains("thing-1/CompA/db/password"));
        assertTrue(raw.contains("thing-1/CompB/db/password"));
    }

    @Test
    void centralSyncFromSecretsManager(@TempDir Path dir) {
        assumeTrue("1".equals(System.getenv("GGCOMMONS_IT_SM")), "needs floci secretsmanager (GGCOMMONS_IT_SM=1)");
        System.setProperty("aws.accessKeyId", "test");
        System.setProperty("aws.secretAccessKey", "test");
        System.setProperty("aws.region", "us-east-1");

        SecretsManagerClient client = SecretsManagerClient.builder()
                .region(Region.US_EAST_1)
                .endpointOverride(URI.create("http://localhost:4566"))
                .build();
        String name = "ggcommons-java-cred-" + UUID.randomUUID();
        client.createSecret(CreateSecretRequest.builder().name(name).secretString("v1").build());
        try {
            JsonObject cfg = config(dir, name);
            CredentialService creds = Credentials.open(cfg); // namespace "" → central id == name
            assertEquals("v1", creds.getString(name).orElseThrow());

            client.putSecretValue(PutSecretValueRequest.builder().secretId(name).secretString("v2").build());
            creds.refresh();
            assertEquals("v2", creds.getString(name).orElseThrow());
            assertTrue(creds.versions(name).size() >= 2); // previous version retained (rotation grace)

            int before = creds.versions(name).size();
            creds.refresh();
            assertEquals(before, creds.versions(name).size()); // no churn when unchanged
        } finally {
            client.deleteSecret(DeleteSecretRequest.builder().secretId(name).forceDeleteWithoutRecovery(true).build());
        }
    }

    private static JsonObject config(Path dir, String name) {
        JsonObject kp = new JsonObject();
        kp.addProperty("type", "file");
        kp.addProperty("keyPath", dir.resolve("vault.key").toString());
        JsonObject vault = new JsonObject();
        vault.addProperty("path", dir.resolve("vault").toString());
        vault.add("keyProvider", kp);

        JsonArray secrets = new JsonArray();
        secrets.add(name);
        JsonObject sync = new JsonObject();
        sync.add("secrets", secrets);
        JsonObject central = new JsonObject();
        central.addProperty("type", "awsSecretsManager");
        central.addProperty("region", "us-east-1");
        central.addProperty("endpointUrl", "http://localhost:4566");
        central.addProperty("bootstrapOnStart", true);
        central.addProperty("refreshIntervalSecs", 0);
        central.add("sync", sync);

        JsonObject cfg = new JsonObject();
        cfg.add("vault", vault);
        cfg.add("central", central);
        return cfg;
    }
}
