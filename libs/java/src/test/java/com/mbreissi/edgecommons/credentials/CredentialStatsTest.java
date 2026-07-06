package com.mbreissi.edgecommons.credentials;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;

import java.nio.charset.StandardCharsets;
import java.nio.file.Path;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

class CredentialStatsTest {

    private static CredentialService svc(Path dir) {
        FileKeyProvider provider = new FileKeyProvider(new byte[32]);
        return new DefaultCredentialService(LocalVault.open(dir.resolve("vault"), provider, 2));
    }

    @Test
    void secretCountTracksPuts(@TempDir Path dir) {
        CredentialService c = svc(dir);
        assertEquals(0, c.stats().secretCount());

        c.put("db/password", "a".getBytes(StandardCharsets.UTF_8));
        c.put("svc/token", "b".getBytes(StandardCharsets.UTF_8));

        CredentialStats stats = c.stats();
        assertEquals(2, stats.secretCount());
        // No central sync configured → no sync stats.
        assertNull(stats.lastSyncAgeMs());
        assertEquals(0, stats.syncFailures());
        assertEquals(0, stats.rotations());
    }
}
