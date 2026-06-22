package com.aws.proserve.ggcommons.parameters;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assumptions.assumeTrue;

import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.Map;
import java.util.Optional;

import org.junit.jupiter.api.Test;

import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.ssm.SsmClient;
import software.amazon.awssdk.services.ssm.model.DeleteParameterRequest;
import software.amazon.awssdk.services.ssm.model.ParameterType;
import software.amazon.awssdk.services.ssm.model.PutParameterRequest;

/**
 * End-to-end {@link AwsSsmSource} vs a local AWS emulator (floci/LocalStack SSM on :4566).
 *
 * <p>Exercises the real SDK path (excluded from unit coverage): seed a String, a SecureString and
 * a 2-key tree, then read them back via the source under test and assert values, the secure flag,
 * version, missing-&gt;empty, and get-by-path. Gated by {@code GGCOMMONS_IT_SSM=1} and skipped
 * otherwise (mirrors {@code CredentialSyncTest}'s secretsmanager gate).
 */
class AwsSsmSourceFlociTest {

    private static final String ENDPOINT =
            System.getenv().getOrDefault("GGCOMMONS_SSM_ENDPOINT", "http://localhost:4566");

    @Test
    void readsStringSecureAndByPath() {
        assumeTrue("1".equals(System.getenv("GGCOMMONS_IT_SSM")), "needs floci ssm (GGCOMMONS_IT_SSM=1)");
        System.setProperty("aws.accessKeyId", "test");
        System.setProperty("aws.secretAccessKey", "test");
        System.setProperty("aws.region", "us-east-1");

        String prefix = "/ggcommons-it-java-" + ProcessHandle.current().pid();

        // --- seed floci via the SDK admin client ---
        SsmClient admin = SsmClient.builder()
                .region(Region.US_EAST_1)
                .endpointOverride(URI.create(ENDPOINT))
                .build();
        put(admin, prefix + "/plain", "us-east-1", ParameterType.STRING);
        put(admin, prefix + "/secure", "p@ss", ParameterType.SECURE_STRING);
        put(admin, prefix + "/tree/a", "1", ParameterType.STRING);
        put(admin, prefix + "/tree/b", "2", ParameterType.STRING);

        try {
            // --- read back via the source under test (withDecryption = true) ---
            AwsSsmSource src = new AwsSsmSource("us-east-1", ENDPOINT, true);

            Optional<ParamValue> plain = src.fetch(prefix + "/plain");
            assertTrue(plain.isPresent(), "plain present");
            assertEquals("us-east-1", new String(plain.get().value(), StandardCharsets.UTF_8));
            assertFalse(plain.get().secure(), "String must not be flagged secure");
            assertTrue(plain.get().version().isPresent(), "version populated");

            Optional<ParamValue> secure = src.fetch(prefix + "/secure");
            assertTrue(secure.isPresent(), "secure present");
            assertEquals("p@ss", new String(secure.get().value(), StandardCharsets.UTF_8), "SecureString decrypts");
            assertTrue(secure.get().secure(), "SecureString flagged secure");

            assertTrue(src.fetch(prefix + "/missing").isEmpty(), "missing -> empty");

            Map<String, String> tree = new HashMap<>();
            for (Map.Entry<String, ParamValue> e : src.fetchByPath(prefix + "/tree", true)) {
                tree.put(e.getKey(), new String(e.getValue().value(), StandardCharsets.UTF_8));
            }
            assertEquals("1", tree.get(prefix + "/tree/a"));
            assertEquals("2", tree.get(prefix + "/tree/b"));
        } finally {
            for (String suffix : new String[] {"/plain", "/secure", "/tree/a", "/tree/b"}) {
                try {
                    admin.deleteParameter(DeleteParameterRequest.builder().name(prefix + suffix).build());
                } catch (RuntimeException ignored) {
                    // best-effort cleanup
                }
            }
        }
    }

    private static void put(SsmClient admin, String name, String value, ParameterType type) {
        admin.putParameter(PutParameterRequest.builder()
                .name(name).value(value).type(type).overwrite(true).build());
    }
}
