package com.breissinger.ggcommons.credentials;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;

import java.nio.charset.StandardCharsets;
import java.nio.file.Path;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/**
 * Unit tests for {@link Secret} accessors via a {@link LocalVault} round-trip: {@code asString}
 * (valid + invalid UTF-8), {@code asJson} (valid + non-JSON), and the redacting {@code toString}.
 */
class SecretTest {

    private static LocalVault vault(Path dir) {
        return LocalVault.open(dir.resolve("vault"), new FileKeyProvider(new byte[32]), 3);
    }

    @Test
    void bytesAndAsStringRoundTrip(@TempDir Path dir) {
        LocalVault v = vault(dir);
        v.put("k", "héllo".getBytes(StandardCharsets.UTF_8), null);
        Secret s = v.get("k");
        assertEquals("héllo", s.asString());
        assertArrayEquals("héllo".getBytes(StandardCharsets.UTF_8), s.bytes());
    }

    @Test
    void asStringThrowsOnInvalidUtf8(@TempDir Path dir) {
        LocalVault v = vault(dir);
        // 0xFF is never a valid UTF-8 byte.
        v.put("bin", new byte[] {(byte) 0xFF, (byte) 0xFE}, null);
        Secret s = v.get("bin");
        CredentialException ex = assertThrows(CredentialException.class, s::asString);
        assertEquals("secret is not valid UTF-8", ex.getMessage());
    }

    @Test
    void asJsonParsesObject(@TempDir Path dir) {
        LocalVault v = vault(dir);
        v.put("cfg", "{\"x\":1}".getBytes(StandardCharsets.UTF_8), null);
        assertEquals(1, v.get("cfg").asJson().getAsJsonObject().get("x").getAsInt());
    }

    @Test
    void asJsonThrowsOnNonJson(@TempDir Path dir) {
        LocalVault v = vault(dir);
        v.put("bad", "{not valid json".getBytes(StandardCharsets.UTF_8), null);
        assertThrows(CredentialException.class, () -> v.get("bad").asJson());
    }

    @Test
    void toStringRedactsValue(@TempDir Path dir) {
        LocalVault v = vault(dir);
        v.put("db/pw", "topsecret".getBytes(StandardCharsets.UTF_8), null);
        String str = v.get("db/pw").toString();
        assertFalse(str.contains("topsecret"), "toString must never leak the secret value");
        assertEquals("Secret{name=db/pw, version=00000001, bytes=<9 redacted>}", str);
    }
}
