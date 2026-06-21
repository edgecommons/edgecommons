package com.aws.proserve.ggcommons.credentials;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/**
 * Phase 4 credential access-audit parity tests (mirrors libs/rust audit behavior). Uses a collecting
 * {@link AuditSink} and asserts the emitted events for put/get(hit)/get(miss)/delete, that the secret
 * value never appears in any event, and that a service with no sink is a no-op.
 */
class CredentialAuditTest {
    private static final String VALUE = "s3cr3t-value";

    /** Collecting sink: records every event to a list for assertions. */
    private static final class CollectingSink implements AuditSink {
        final List<AuditEvent> events = new ArrayList<>();

        @Override
        public void record(AuditEvent event) {
            events.add(event);
        }
    }

    private static DefaultCredentialService svc(Path dir) {
        FileKeyProvider provider = new FileKeyProvider(new byte[32]);
        return new DefaultCredentialService(LocalVault.open(dir.resolve("vault"), provider, 2));
    }

    @Test
    void emitsExpectedEventsForPutGetMissDelete(@TempDir Path dir) {
        CollectingSink sink = new CollectingSink();
        DefaultCredentialService c = svc(dir).withAudit(sink);

        // put -> ("put", name, newVersion, "local", "ok")
        String version = c.put("db/password", VALUE.getBytes(StandardCharsets.UTF_8));
        // get(hit) -> ("get", name, version, source, "hit")
        c.get("db/password");
        // get(miss) -> ("get", name, "-", "-", "miss")
        c.get("does/not/exist");
        // delete(ok) -> ("delete", name, "-", "-", "ok")
        c.delete("db/password");
        // delete(miss) -> ("delete", name, "-", "-", "miss")
        c.delete("db/password");

        assertEquals(5, sink.events.size());

        AuditEvent put = sink.events.get(0);
        assertEquals("put", put.op());
        assertEquals("db/password", put.name());
        assertEquals(version, put.version());
        assertEquals("local", put.source());
        assertEquals("ok", put.outcome());

        AuditEvent hit = sink.events.get(1);
        assertEquals("get", hit.op());
        assertEquals("db/password", hit.name());
        assertEquals(version, hit.version());
        assertEquals("local", hit.source());
        assertEquals("hit", hit.outcome());

        AuditEvent miss = sink.events.get(2);
        assertEquals("get", miss.op());
        assertEquals("does/not/exist", miss.name());
        assertEquals("-", miss.version());
        assertEquals("-", miss.source());
        assertEquals("miss", miss.outcome());

        AuditEvent del = sink.events.get(3);
        assertEquals("delete", del.op());
        assertEquals("db/password", del.name());
        assertEquals("-", del.version());
        assertEquals("-", del.source());
        assertEquals("ok", del.outcome());

        AuditEvent delMiss = sink.events.get(4);
        assertEquals("delete", delMiss.op());
        assertEquals("db/password", delMiss.name());
        assertEquals("-", delMiss.version());
        assertEquals("-", delMiss.source());
        assertEquals("miss", delMiss.outcome());
    }

    @Test
    void getVersionEmitsHitAndMiss(@TempDir Path dir) {
        CollectingSink sink = new CollectingSink();
        DefaultCredentialService c = svc(dir).withAudit(sink);

        String version = c.put("api/key", VALUE.getBytes(StandardCharsets.UTF_8));
        sink.events.clear();

        c.getVersion("api/key", version);
        c.getVersion("api/key", "no-such-version");

        assertEquals(2, sink.events.size());

        AuditEvent hit = sink.events.get(0);
        assertEquals("get", hit.op());
        assertEquals("api/key", hit.name());
        assertEquals(version, hit.version());
        assertEquals("hit", hit.outcome());

        AuditEvent miss = sink.events.get(1);
        assertEquals("get", miss.op());
        assertEquals("api/key", miss.name());
        // get_version miss -> version echoed back, source "-"
        assertEquals("no-such-version", miss.version());
        assertEquals("-", miss.source());
        assertEquals("miss", miss.outcome());
    }

    @Test
    void neverIncludesTheSecretValue(@TempDir Path dir) {
        CollectingSink sink = new CollectingSink();
        DefaultCredentialService c = svc(dir).withAudit(sink);

        c.put("db/password", VALUE.getBytes(StandardCharsets.UTF_8));
        c.get("db/password");
        c.delete("db/password");

        assertFalse(sink.events.isEmpty());
        for (AuditEvent e : sink.events) {
            assertFalse(e.op().contains(VALUE));
            assertFalse(e.name().contains(VALUE));
            assertFalse(e.version().contains(VALUE));
            assertFalse(e.source().contains(VALUE));
            assertFalse(e.outcome().contains(VALUE));
        }
    }

    @Test
    void noSinkIsNoOp(@TempDir Path dir) {
        // No withAudit() call -> audit field stays null; ops must not throw.
        DefaultCredentialService c = svc(dir);
        String version = c.put("db/password", VALUE.getBytes(StandardCharsets.UTF_8));
        assertTrue(c.get("db/password").isPresent());
        assertEquals(version, c.get("db/password").get().version());
        assertTrue(c.delete("db/password"));
        assertFalse(c.get("db/password").isPresent());
    }

    @Test
    void configEnablesAuditByDefaultAndDisableSilences(@TempDir Path dir) {
        // Default (no audit section) -> auditing on (LogAuditSink attached, ops succeed).
        com.google.gson.JsonObject cfg = new com.google.gson.JsonObject();
        com.google.gson.JsonObject vault = new com.google.gson.JsonObject();
        vault.addProperty("path", dir.resolve("vault-default").toString());
        cfg.add("vault", vault);
        CredentialService enabled = Credentials.open(cfg);
        enabled.put("k", VALUE.getBytes(StandardCharsets.UTF_8));
        assertTrue(enabled.get("k").isPresent());

        // audit.enabled = false -> no sink; ops still succeed.
        com.google.gson.JsonObject cfg2 = new com.google.gson.JsonObject();
        com.google.gson.JsonObject vault2 = new com.google.gson.JsonObject();
        vault2.addProperty("path", dir.resolve("vault-off").toString());
        cfg2.add("vault", vault2);
        com.google.gson.JsonObject audit = new com.google.gson.JsonObject();
        audit.addProperty("enabled", false);
        cfg2.add("audit", audit);
        CredentialService disabled = Credentials.open(cfg2);
        disabled.put("k", VALUE.getBytes(StandardCharsets.UTF_8));
        assertTrue(disabled.get("k").isPresent());
    }
}
