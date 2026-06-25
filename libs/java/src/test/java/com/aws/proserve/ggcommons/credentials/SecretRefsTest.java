package com.aws.proserve.ggcommons.credentials;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.charset.StandardCharsets;
import java.nio.file.Path;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;

class SecretRefsTest {

    private static CredentialService svc(Path dir) {
        FileKeyProvider provider = new FileKeyProvider(new byte[32]);
        return new DefaultCredentialService(LocalVault.open(dir.resolve("vault"), provider, 2));
    }

    @Test
    void resolvesWholeValueAndField(@TempDir Path dir) {
        CredentialService c = svc(dir);
        c.put("db/password", "s3cr3t".getBytes(StandardCharsets.UTF_8));
        c.put("aws", "{\"accessKeyId\":\"AKIA\",\"secretAccessKey\":\"sk\"}".getBytes(StandardCharsets.UTF_8));

        JsonObject cfg = JsonParser.parseString(
                "{\"password\":{\"$secret\":\"db/password\"},"
                + "\"key\":{\"$secret\":\"aws\",\"field\":\"accessKeyId\"},"
                + "\"nested\":{\"arr\":[{\"$secret\":\"db/password\"},\"plain\"]}}").getAsJsonObject();

        JsonElement resolved = SecretRefs.resolve(cfg, c);
        JsonObject out = resolved.getAsJsonObject();
        assertEquals("s3cr3t", out.get("password").getAsString());
        assertEquals("AKIA", out.get("key").getAsString());
        assertEquals("s3cr3t", out.getAsJsonObject("nested").getAsJsonArray("arr").get(0).getAsString());
        assertEquals("plain", out.getAsJsonObject("nested").getAsJsonArray("arr").get(1).getAsString());

        // Original config is not mutated.
        assertEquals("db/password", cfg.getAsJsonObject("password").get("$secret").getAsString());
    }

    @Test
    void missingSecretThrows(@TempDir Path dir) {
        CredentialService c = svc(dir);
        JsonObject cfg = JsonParser.parseString("{\"x\":{\"$secret\":\"nope\"}}").getAsJsonObject();
        assertThrows(CredentialException.class, () -> SecretRefs.resolve(cfg, c));
    }

    @Test
    void missingFieldThrows(@TempDir Path dir) {
        CredentialService c = svc(dir);
        c.put("aws", "{\"accessKeyId\":\"AKIA\"}".getBytes(StandardCharsets.UTF_8));
        JsonObject cfg = JsonParser.parseString("{\"x\":{\"$secret\":\"aws\",\"field\":\"missing\"}}").getAsJsonObject();
        assertThrows(CredentialException.class, () -> SecretRefs.resolve(cfg, c));
    }

    @Test
    void nullAndJsonNullPassThroughUnchanged(@TempDir Path dir) {
        CredentialService c = svc(dir);
        assertNull(SecretRefs.resolve(null, c));
        JsonElement jsonNull = com.google.gson.JsonNull.INSTANCE;
        assertEquals(jsonNull, SecretRefs.resolve(jsonNull, c));
    }

    @Test
    void plainPrimitivesAndArraysUnchanged(@TempDir Path dir) {
        CredentialService c = svc(dir);
        JsonElement prim = new com.google.gson.JsonPrimitive("just a string");
        assertEquals("just a string", SecretRefs.resolve(prim, c).getAsString());

        JsonElement arr = JsonParser.parseString("[1,2,\"three\"]");
        JsonElement out = SecretRefs.resolve(arr, c);
        assertEquals(3, out.getAsJsonArray().size());
        assertEquals("three", out.getAsJsonArray().get(2).getAsString());
    }

    @Test
    void fieldRequestedOnNonObjectSecretThrows(@TempDir Path dir) {
        CredentialService c = svc(dir);
        // The stored secret is a bare JSON string, not an object → requesting a field must fail.
        c.put("plain", "\"just-a-string\"".getBytes(StandardCharsets.UTF_8));
        JsonObject cfg = JsonParser.parseString("{\"x\":{\"$secret\":\"plain\",\"field\":\"k\"}}").getAsJsonObject();
        CredentialException ex = assertThrows(CredentialException.class, () -> SecretRefs.resolve(cfg, c));
        assertTrue(ex.getMessage().contains("field 'k'"));
    }

    @Test
    void fieldThatIsNotAStringThrows(@TempDir Path dir) {
        CredentialService c = svc(dir);
        c.put("cfg", "{\"port\":8080}".getBytes(StandardCharsets.UTF_8));
        JsonObject cfg = JsonParser.parseString("{\"x\":{\"$secret\":\"cfg\",\"field\":\"port\"}}").getAsJsonObject();
        assertThrows(CredentialException.class, () -> SecretRefs.resolve(cfg, c));
    }

    @Test
    void nonPrimitiveSecretMarkerIsTreatedAsPlainObject(@TempDir Path dir) {
        CredentialService c = svc(dir);
        // {"$secret": {...}} — the marker value is not a primitive, so it is NOT a secret ref; the
        // object is recursed into as ordinary config (and the inner object has no $secret marker).
        JsonObject cfg = JsonParser.parseString("{\"$secret\":{\"nested\":\"x\"}}").getAsJsonObject();
        JsonElement out = SecretRefs.resolve(cfg, c);
        assertEquals("x", out.getAsJsonObject().getAsJsonObject("$secret").get("nested").getAsString());
    }
}
