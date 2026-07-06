package com.mbreissi.edgecommons.credentials;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assumptions.assumeTrue;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Base64;
import java.util.List;
import java.util.TreeMap;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import com.mbreissi.edgecommons.credentials.VaultModel.SecretEntry;
import com.mbreissi.edgecommons.credentials.VaultModel.VersionEntry;
import com.google.gson.Gson;
import com.google.gson.JsonObject;

class VaultTest {
    private static final Base64.Decoder B64D = Base64.getDecoder();
    private static final Base64.Encoder B64E = Base64.getEncoder();
    private static final Path VECTORS = Path.of("..", "..", "vault-test-vectors");

    private static CredentialService svc(Path dir) {
        FileKeyProvider provider = new FileKeyProvider(new byte[32]);
        return new DefaultCredentialService(LocalVault.open(dir.resolve("vault"), provider, 2));
    }

    @Test
    void putGetRoundtripAndTypedViews(@TempDir Path dir) {
        CredentialService c = svc(dir);
        c.put("db/password", "s3cr3t".getBytes(StandardCharsets.UTF_8));
        c.put("svc/config", "{\"k\":1}".getBytes(StandardCharsets.UTF_8));
        assertEquals("s3cr3t", c.getString("db/password").orElseThrow());
        assertEquals(1, c.getJson("svc/config").orElseThrow().getAsJsonObject().get("k").getAsInt());
        assertTrue(c.exists("db/password"));
        assertTrue(c.get("missing").isEmpty());
        List<String> names = new ArrayList<>();
        c.list("").forEach(m -> names.add(m.name()));
        assertEquals(List.of("db/password", "svc/config"), names);
    }

    @Test
    void typedViews(@TempDir Path dir) {
        CredentialService c = svc(dir);
        c.put("aws", "{\"accessKeyId\":\"AKIA\",\"secretAccessKey\":\"sk\",\"sessionToken\":\"tok\"}".getBytes(StandardCharsets.UTF_8));
        c.put("basic", "{\"username\":\"u\",\"password\":\"p\"}".getBytes(StandardCharsets.UTF_8));
        c.put("tls", "{\"certPem\":\"C\",\"keyPem\":\"K\"}".getBytes(StandardCharsets.UTF_8));
        c.put("kafka", "{\"username\":\"ku\",\"password\":\"kp\"}".getBytes(StandardCharsets.UTF_8));
        assertEquals("AKIA", c.getAwsCredentials("aws").orElseThrow().accessKeyId());
        assertEquals("tok", c.getAwsCredentials("aws").orElseThrow().sessionToken());
        assertEquals("u", c.getBasicAuth("basic").orElseThrow().username());
        assertEquals("C", c.getTlsBundle("tls").orElseThrow().certPem());
        assertEquals("PLAIN", c.getKafkaSasl("kafka").orElseThrow().mechanism()); // default
        assertThrows(CredentialException.class, () -> c.getAwsCredentials("basic")); // wrong shape
    }

    @Test
    void versionsMonotonicAndPruned(@TempDir Path dir) {
        CredentialService c = svc(dir); // keep_versions = 2
        c.put("k", "v1".getBytes());
        c.put("k", "v2".getBytes());
        c.put("k", "v3".getBytes());
        assertEquals(List.of("00000002", "00000003"), c.versions("k"));
        assertEquals("v3", c.get("k").orElseThrow().asString());
        assertEquals("v2", c.getVersion("k", "00000002").orElseThrow().asString());
        assertTrue(c.getVersion("k", "00000001").isEmpty());
    }

    @Test
    void persistsAndReopens(@TempDir Path dir) {
        svc(dir).put("token", "abc".getBytes());
        assertEquals("abc", svc(dir).getString("token").orElseThrow());
    }

    @Test
    void wrongKekFailsClosed(@TempDir Path dir) {
        svc(dir).put("token", "abc".getBytes());
        byte[] other = new byte[32];
        java.util.Arrays.fill(other, (byte) 9);
        assertThrows(CredentialException.class,
                () -> LocalVault.open(dir.resolve("vault"), new FileKeyProvider(other), 2));
    }

    @Test
    void tamperDetected(@TempDir Path dir) throws Exception {
        svc(dir).put("k", "v1".getBytes());
        Path path = dir.resolve("vault");
        Gson gson = new Gson();
        JsonObject vf = gson.fromJson(Files.readString(path), JsonObject.class);
        JsonObject ver = vf.getAsJsonObject("secrets").getAsJsonObject("k")
                .getAsJsonArray("versions").get(0).getAsJsonObject();
        byte[] ct = B64D.decode(ver.get("ciphertext").getAsString());
        ct[0] ^= 0x01;
        ver.addProperty("ciphertext", B64E.encodeToString(ct));
        Files.writeString(path, gson.toJson(vf));
        assertThrows(CredentialException.class,
                () -> LocalVault.open(path, new FileKeyProvider(new byte[32]), 2));
    }

    @Test
    void crossLanguageConformance() throws Exception {
        assumeTrue(Files.exists(VECTORS.resolve("vault.json")), "vault-test-vectors not present");
        Gson gson = new Gson();
        JsonObject vec = gson.fromJson(Files.readString(VECTORS.resolve("vectors.json")), JsonObject.class);
        byte[] kek = B64D.decode(vec.get("kekB64").getAsString());
        byte[] dek = B64D.decode(vec.get("dekB64").getAsString());
        String vaultId = vec.get("vaultId").getAsString();

        // (1) decrypt the canonical (Rust-generated) vault using the committed key file
        FileKeyProvider provider = FileKeyProvider.fromKeyFile(VECTORS.resolve("vault.key"));
        LocalVault v = LocalVault.open(VECTORS.resolve("vault.json"), provider, 2);
        assertArrayEquals("hello".getBytes(StandardCharsets.UTF_8), v.get("alpha").bytes());
        assertEquals(1, v.get("beta").asJson().getAsJsonObject().get("x").getAsInt());

        // (2) reproduce the wrapped DEK
        byte[] wrapped = VaultCrypto.seal(kek, B64D.decode(vec.get("wrapNonceB64").getAsString()),
                VaultFormat.dekWrapAad(vaultId), dek);
        assertEquals(vec.get("wrappedDekB64").getAsString(), B64E.encodeToString(wrapped));

        // (3) reproduce each record ciphertext + build the secrets map for the MAC
        TreeMap<String, SecretEntry> secrets = new TreeMap<>();
        for (var el : vec.getAsJsonArray("records")) {
            JsonObject r = el.getAsJsonObject();
            String name = r.get("name").getAsString();
            String version = r.get("version").getAsString();
            byte[] nonce = B64D.decode(r.get("nonceB64").getAsString());
            byte[] pt = B64D.decode(r.get("plaintextB64").getAsString());
            byte[] ct = VaultCrypto.seal(dek, nonce, VaultFormat.recordAad(vaultId, name, version), pt);
            assertEquals(r.get("ciphertextB64").getAsString(), B64E.encodeToString(ct), name);
            VersionEntry ve = new VersionEntry();
            ve.version = version;
            ve.createdMs = 1_700_000_000_000L;
            ve.source = "local";
            ve.contentType = "application/octet-stream";
            ve.nonce = r.get("nonceB64").getAsString();
            ve.ciphertext = r.get("ciphertextB64").getAsString();
            SecretEntry se = new SecretEntry();
            se.versions = new ArrayList<>(List.of(ve));
            secrets.put(name, se);
        }

        // (4) reproduce the MAC over the canonical byte string
        byte[] macKey = VaultCrypto.deriveMacKey(dek, vaultId.getBytes(StandardCharsets.UTF_8));
        String mac = B64E.encodeToString(VaultCrypto.hmac(macKey, VaultFormat.macInput(vaultId, secrets)));
        assertEquals(vec.get("macB64").getAsString(), mac);

        assertFalse(false);
    }
}
