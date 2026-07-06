package com.mbreissi.edgecommons.credentials;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;

import org.junit.jupiter.api.Test;

import com.google.gson.JsonElement;

/**
 * Unit tests for the {@link CredentialService} typed views
 * ({@code getAwsCredentials/getBasicAuth/getTlsBundle/getKafkaSasl}), the {@link Secret} value
 * accessors ({@code asString/asJson/bytes/toString}-redaction) and the credential value records.
 *
 * <p>These exercise the pure parse/redaction logic with no vault, no key provider and no AWS — a
 * tiny in-memory {@link CredentialService} backed by a map of {@link Secret} objects (constructed via
 * the package-private constructor) drives the default-method typed views.
 */
class SecretViewsTest {

    /** Minimal in-memory credential service: only {@link #get(String)} is wired; the typed-view
     * default methods build on it. Write/list operations are unsupported. */
    private static final class FakeCreds implements CredentialService {
        private final Map<String, Secret> store = new HashMap<>();

        /** Store a secret whose value is the given UTF-8 string. */
        void seedString(String name, String value) {
            store.put(name, secret(name, value.getBytes(StandardCharsets.UTF_8)));
        }

        /** Store a secret whose value is the given raw bytes. */
        void seedBytes(String name, byte[] value) {
            store.put(name, secret(name, value));
        }

        @Override
        public Optional<Secret> get(String name) {
            return Optional.ofNullable(store.get(name));
        }

        @Override
        public Optional<Secret> getVersion(String name, String version) {
            return get(name);
        }

        @Override
        public boolean exists(String name) {
            return store.containsKey(name);
        }

        @Override
        public List<SecretMeta> list(String prefix) {
            throw new UnsupportedOperationException();
        }

        @Override
        public List<String> versions(String name) {
            throw new UnsupportedOperationException();
        }

        @Override
        public String put(String name, byte[] value, PutOptions opts) {
            throw new UnsupportedOperationException();
        }

        @Override
        public boolean delete(String name) {
            throw new UnsupportedOperationException();
        }
    }

    private static Secret secret(String name, byte[] value) {
        return new Secret(name, "v1", value, Map.of(), 123L, "local", "application/octet-stream");
    }

    // ---------- AWS credentials view ----------

    @Test
    void awsCredentialsParsesFullAndPartial() {
        FakeCreds c = new FakeCreds();
        c.seedString("aws",
                "{\"accessKeyId\":\"AKIA\",\"secretAccessKey\":\"sk\","
                + "\"sessionToken\":\"tok\",\"expiry\":\"2030-01-01T00:00:00Z\"}");
        AwsCredentials v = c.getAwsCredentials("aws").orElseThrow();
        assertEquals("AKIA", v.accessKeyId());
        assertEquals("sk", v.secretAccessKey());
        assertEquals("tok", v.sessionToken());
        assertEquals("2030-01-01T00:00:00Z", v.expiry());

        // Optional fields may be absent.
        c.seedString("aws2", "{\"accessKeyId\":\"AKIA\",\"secretAccessKey\":\"sk\"}");
        AwsCredentials v2 = c.getAwsCredentials("aws2").orElseThrow();
        assertNull(v2.sessionToken());
        assertNull(v2.expiry());
    }

    @Test
    void awsCredentialsAbsentSecretIsEmptyOptional() {
        assertTrue(new FakeCreds().getAwsCredentials("missing").isEmpty());
    }

    @Test
    void awsCredentialsMissingRequiredFieldThrows() {
        FakeCreds c = new FakeCreds();
        c.seedString("aws", "{\"accessKeyId\":\"AKIA\"}"); // no secretAccessKey
        CredentialException ex = assertThrows(CredentialException.class, () -> c.getAwsCredentials("aws"));
        assertTrue(ex.getMessage().contains("not AWS credentials"));
        // The message must not leak the value.
        assertFalse(ex.getMessage().contains("AKIA"));
    }

    @Test
    void awsCredentialsMalformedJsonThrows() {
        FakeCreds c = new FakeCreds();
        c.seedString("aws", "{not json");
        assertThrows(CredentialException.class, () -> c.getAwsCredentials("aws"));
    }

    // ---------- Basic auth view ----------

    @Test
    void basicAuthParses() {
        FakeCreds c = new FakeCreds();
        c.seedString("ba", "{\"username\":\"u\",\"password\":\"p\"}");
        BasicAuth v = c.getBasicAuth("ba").orElseThrow();
        assertEquals("u", v.username());
        assertEquals("p", v.password());
    }

    @Test
    void basicAuthMissingPasswordThrows() {
        FakeCreds c = new FakeCreds();
        c.seedString("ba", "{\"username\":\"u\"}");
        assertThrows(CredentialException.class, () -> c.getBasicAuth("ba"));
    }

    // ---------- TLS bundle view ----------

    @Test
    void tlsBundleParsesWithOptionalCa() {
        FakeCreds c = new FakeCreds();
        c.seedString("tls", "{\"certPem\":\"CERT\",\"keyPem\":\"KEY\",\"caPem\":\"CA\"}");
        TlsBundle v = c.getTlsBundle("tls").orElseThrow();
        assertEquals("CERT", v.certPem());
        assertEquals("KEY", v.keyPem());
        assertEquals("CA", v.caPem());

        // caPem is optional.
        c.seedString("tls2", "{\"certPem\":\"CERT\",\"keyPem\":\"KEY\"}");
        assertNull(c.getTlsBundle("tls2").orElseThrow().caPem());
    }

    @Test
    void tlsBundleMissingKeyThrows() {
        FakeCreds c = new FakeCreds();
        c.seedString("tls", "{\"certPem\":\"CERT\"}");
        assertThrows(CredentialException.class, () -> c.getTlsBundle("tls"));
    }

    // ---------- Kafka SASL view ----------

    @Test
    void kafkaSaslDefaultsMechanismToPlainWhenAbsent() {
        FakeCreds c = new FakeCreds();
        c.seedString("k", "{\"username\":\"u\",\"password\":\"p\"}");
        KafkaSasl v = c.getKafkaSasl("k").orElseThrow();
        assertEquals("PLAIN", v.mechanism());
        assertEquals("u", v.username());
        assertEquals("p", v.password());
    }

    @Test
    void kafkaSaslKeepsExplicitMechanism() {
        FakeCreds c = new FakeCreds();
        c.seedString("k", "{\"mechanism\":\"SCRAM-SHA-512\",\"username\":\"u\",\"password\":\"p\"}");
        assertEquals("SCRAM-SHA-512", c.getKafkaSasl("k").orElseThrow().mechanism());
    }

    @Test
    void kafkaSaslMissingUsernameThrows() {
        FakeCreds c = new FakeCreds();
        c.seedString("k", "{\"password\":\"p\"}");
        assertThrows(CredentialException.class, () -> c.getKafkaSasl("k"));
    }

    // ---------- Secret accessors ----------

    @Test
    void secretAsStringDecodesUtf8() {
        Secret s = secret("n", "héllo".getBytes(StandardCharsets.UTF_8));
        assertEquals("héllo", s.asString());
    }

    @Test
    void secretAsStringRejectsInvalidUtf8() {
        // 0xFF is never a valid UTF-8 byte.
        Secret s = secret("n", new byte[] {(byte) 0xFF, (byte) 0xFE});
        CredentialException ex = assertThrows(CredentialException.class, s::asString);
        assertTrue(ex.getMessage().contains("not valid UTF-8"));
    }

    @Test
    void secretAsJsonParsesAndRejectsBadJson() {
        Secret ok = secret("n", "{\"a\":1}".getBytes(StandardCharsets.UTF_8));
        JsonElement json = ok.asJson();
        assertEquals(1, json.getAsJsonObject().get("a").getAsInt());

        Secret bad = secret("n", "{nope".getBytes(StandardCharsets.UTF_8));
        assertThrows(CredentialException.class, bad::asJson);
    }

    @Test
    void secretBytesAndMetadataAccessors() {
        byte[] raw = {1, 2, 3};
        Secret s = secret("db/pw", raw);
        assertArrayEquals(raw, s.bytes());
        assertEquals("db/pw", s.name());
        assertEquals("v1", s.version());
        assertEquals(123L, s.createdMs());
        assertEquals("local", s.source());
        assertEquals("application/octet-stream", s.contentType());
        assertTrue(s.labels().isEmpty());
    }

    @Test
    void secretToStringRedactsTheValue() {
        Secret s = secret("db/pw", "TOP-SECRET-VALUE".getBytes(StandardCharsets.UTF_8));
        String str = s.toString();
        assertFalse(str.contains("TOP-SECRET-VALUE"), "toString must not leak the value");
        assertTrue(str.contains("redacted"));
        assertTrue(str.contains("db/pw"));
        assertTrue(str.contains("v1"));
    }

    // ---------- Default-method bytes/string/json convenience over get() ----------

    @Test
    void getBytesStringJsonConvenience() {
        FakeCreds c = new FakeCreds();
        c.seedBytes("raw", new byte[] {9, 8, 7});
        assertArrayEquals(new byte[] {9, 8, 7}, c.getBytes("raw").orElseThrow());

        c.seedString("s", "hello");
        assertEquals("hello", c.getString("s").orElseThrow());

        c.seedString("j", "{\"k\":\"val\"}");
        assertEquals("val", c.getJson("j").orElseThrow().getAsJsonObject().get("k").getAsString());

        assertTrue(c.getString("missing").isEmpty());
    }

    // ---------- Value records: equality / accessors ----------

    @Test
    void valueRecordsExposeComponents() {
        AwsCredentials aws = new AwsCredentials("ak", "sk", "st", "exp");
        assertEquals("ak", aws.accessKeyId());
        assertEquals(aws, new AwsCredentials("ak", "sk", "st", "exp"));

        BasicAuth ba = new BasicAuth("u", "p");
        assertEquals("u", ba.username());
        assertEquals(ba, new BasicAuth("u", "p"));

        TlsBundle tls = new TlsBundle("c", "k", "ca");
        assertEquals("ca", tls.caPem());

        KafkaSasl sasl = new KafkaSasl("PLAIN", "u", "p");
        assertEquals("PLAIN", sasl.mechanism());
    }

    @Test
    void centralSecretAndStatsRecords() {
        CentralSecret cs = new CentralSecret(new byte[] {1, 2}, "ver-1", Map.of("env", "prod"));
        assertArrayEquals(new byte[] {1, 2}, cs.bytes());
        assertEquals("ver-1", cs.centralVersionId());
        assertEquals("prod", cs.labels().get("env"));

        CredentialStats stats = new CredentialStats(5, 100L, 2, 3);
        assertEquals(5, stats.secretCount());
        assertEquals(100L, stats.lastSyncAgeMs());
        assertEquals(2, stats.syncFailures());
        assertEquals(3, stats.rotations());
    }

    @Test
    void credentialExceptionCarriesMessageAndCause() {
        CredentialException e1 = new CredentialException("boom");
        assertEquals("boom", e1.getMessage());
        assertNull(e1.getCause());

        Throwable cause = new IllegalStateException("root");
        CredentialException e2 = new CredentialException("wrapped", cause);
        assertEquals("wrapped", e2.getMessage());
        assertEquals(cause, e2.getCause());
    }
}
