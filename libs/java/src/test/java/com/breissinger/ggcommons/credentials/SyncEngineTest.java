package com.breissinger.ggcommons.credentials;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.function.Function;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/**
 * Unit tests for {@link SyncEngine} using a programmable in-memory {@link CentralVaultSource} (no
 * AWS). Covers bootstrap seeding, change detection via {@code centralVersionId}, the offline
 * keep-cached-on-failure path + failure counter, rotation counting, the {@code from} central-id
 * override, and namespacing of the local key.
 */
class SyncEngineTest {

    /** A scriptable central source: a function from central-id to an optional secret. */
    private static final class FakeSource implements CentralVaultSource {
        final AtomicInteger calls = new AtomicInteger();
        volatile Function<String, Optional<CentralSecret>> handler = id -> Optional.empty();

        @Override
        public Optional<CentralSecret> fetch(String name) {
            calls.incrementAndGet();
            return handler.apply(name);
        }
    }

    private static LocalVault vault(Path dir) {
        return LocalVault.open(dir.resolve("vault"), new FileKeyProvider(new byte[32]), 2);
    }

    private static CentralSecret secret(String value, String version) {
        return new CentralSecret(value.getBytes(StandardCharsets.UTF_8), version, Map.of());
    }

    @Test
    void bootstrapSeedsTheVaultSynchronously(@TempDir Path dir) {
        LocalVault v = vault(dir);
        Object lock = new Object();
        FakeSource src = new FakeSource();
        src.handler = id -> "db/password".equals(id) ? Optional.of(secret("v1", "c1")) : Optional.empty();

        try (SyncEngine engine = new SyncEngine(v, lock, src, "", List.of(
                new SyncEngine.SyncSecret("db/password", null)), 0, true)) {
            // bootstrap=true ran syncNow() in the constructor
            assertEquals("v1", v.get("db/password").asString());
            assertEquals(1, src.calls.get());
            SyncEngine.SyncStats s = engine.stats();
            assertNotNull(s.lastSuccessMs());
            assertEquals(0, s.failures());
            assertEquals(1, s.rotations());
        }
    }

    @Test
    void noBootstrapLeavesVaultUntilSyncNow(@TempDir Path dir) {
        LocalVault v = vault(dir);
        FakeSource src = new FakeSource();
        src.handler = id -> Optional.of(secret("v1", "c1"));

        try (SyncEngine engine = new SyncEngine(v, new Object(), src, "", List.of(
                new SyncEngine.SyncSecret("k", null)), 0, false)) {
            assertNull(v.get("k"));
            assertNull(engine.stats().lastSuccessMs());

            engine.syncNow();
            assertEquals("v1", v.get("k").asString());
            assertNotNull(engine.stats().lastSuccessMs());
        }
    }

    @Test
    void unchangedVersionDoesNotRewriteOrRotate(@TempDir Path dir) {
        LocalVault v = vault(dir);
        FakeSource src = new FakeSource();
        src.handler = id -> Optional.of(secret("same", "ver-A"));

        try (SyncEngine engine = new SyncEngine(v, new Object(), src, "", List.of(
                new SyncEngine.SyncSecret("k", null)), 0, true)) {
            assertEquals(1, engine.stats().rotations());
            List<String> afterFirst = v.versions("k");

            engine.syncNow(); // same centralVersionId -> no-op
            assertEquals(1, engine.stats().rotations());
            assertEquals(afterFirst, v.versions("k"));
        }
    }

    @Test
    void changedVersionWritesNewVersionAndIncrementsRotations(@TempDir Path dir) {
        LocalVault v = vault(dir);
        FakeSource src = new FakeSource();
        src.handler = id -> Optional.of(secret("v1", "ver-A"));

        try (SyncEngine engine = new SyncEngine(v, new Object(), src, "", List.of(
                new SyncEngine.SyncSecret("k", null)), 0, true)) {
            assertEquals(1, engine.stats().rotations());

            src.handler = id -> Optional.of(secret("v2", "ver-B"));
            engine.syncNow();

            assertEquals(2, engine.stats().rotations());
            assertEquals("v2", v.get("k").asString());
            assertEquals(2, v.versions("k").size());
        }
    }

    @Test
    void emptyUpstreamSecretIsSkippedButCountsAsSuccess(@TempDir Path dir) {
        LocalVault v = vault(dir);
        FakeSource src = new FakeSource();
        src.handler = id -> Optional.empty();

        try (SyncEngine engine = new SyncEngine(v, new Object(), src, "", List.of(
                new SyncEngine.SyncSecret("k", null)), 0, true)) {
            assertNull(v.get("k"));
            assertEquals(0, engine.stats().rotations());
            assertEquals(0, engine.stats().failures());
            // fetch succeeded (returned empty) -> lastSuccess set
            assertNotNull(engine.stats().lastSuccessMs());
        }
    }

    @Test
    void fetchFailureKeepsCachedValueAndIncrementsFailures(@TempDir Path dir) {
        LocalVault v = vault(dir);
        Object lock = new Object();
        FakeSource src = new FakeSource();

        // First a successful seed.
        src.handler = id -> Optional.of(secret("cached", "ver-A"));
        try (SyncEngine engine = new SyncEngine(v, lock, src, "", List.of(
                new SyncEngine.SyncSecret("k", null)), 0, true)) {
            assertEquals("cached", v.get("k").asString());
            Long firstSuccess = engine.stats().lastSuccessMs();

            // Now the source throws -> offline-first keeps the cached value.
            src.handler = id -> {
                throw new RuntimeException("network down");
            };
            engine.syncNow();

            assertEquals("cached", v.get("k").asString()); // unchanged
            assertEquals(1, engine.stats().failures());
            // No successful pass this time -> lastSuccess unchanged.
            assertEquals(firstSuccess, engine.stats().lastSuccessMs());
        }
    }

    @Test
    void fromOverrideUsesSharedCentralIdNotNamespacedKey(@TempDir Path dir) {
        LocalVault v = vault(dir);
        FakeSource src = new FakeSource();
        // Only respond to the shared central id; the namespaced local key would return empty.
        src.handler = id -> "shared/db".equals(id) ? Optional.of(secret("shared-val", "c1")) : Optional.empty();

        try (SyncEngine engine = new SyncEngine(v, new Object(), src, "thing-1/CompA", List.of(
                new SyncEngine.SyncSecret("db/password", "shared/db")), 0, true)) {
            // Stored under the namespaced local key.
            assertEquals("shared-val", v.get("thing-1/CompA/db/password").asString());
            assertEquals(1, engine.stats().rotations());
        }
    }

    @Test
    void withoutFromCentralIdIsTheNamespacedLocalKey(@TempDir Path dir) {
        LocalVault v = vault(dir);
        FakeSource src = new FakeSource();
        // central id defaults to namespaced path "ns/k"
        src.handler = id -> "ns/k".equals(id) ? Optional.of(secret("v", "c1")) : Optional.empty();

        try (SyncEngine engine = new SyncEngine(v, new Object(), src, "ns", List.of(
                new SyncEngine.SyncSecret("k", null)), 0, true)) {
            assertEquals("v", v.get("ns/k").asString());
            assertEquals(1, engine.stats().rotations());
        }
    }

    @Test
    void statsNeverSyncedReportsNullLastSuccess(@TempDir Path dir) {
        LocalVault v = vault(dir);
        FakeSource src = new FakeSource();
        try (SyncEngine engine = new SyncEngine(v, new Object(), src, "", List.of(), 0, false)) {
            SyncEngine.SyncStats s = engine.stats();
            assertNull(s.lastSuccessMs());
            assertEquals(0, s.failures());
            assertEquals(0, s.rotations());
            assertEquals(0, src.calls.get());
        }
    }

    @Test
    void closeIsIdempotentWithoutScheduler(@TempDir Path dir) {
        LocalVault v = vault(dir);
        FakeSource src = new FakeSource();
        SyncEngine engine = new SyncEngine(v, new Object(), src, "", List.of(), 0, false);
        engine.close();
        engine.close(); // no scheduler -> safe to call again
        assertTrue(true);
    }

    @Test
    void scheduledRefreshFiresAtLeastOnce(@TempDir Path dir) throws Exception {
        LocalVault v = vault(dir);
        FakeSource src = new FakeSource();
        src.handler = id -> Optional.of(secret("v", "c1"));

        // intervalSecs > 0 starts the daemon scheduler; bootstrap also runs once synchronously.
        try (SyncEngine engine = new SyncEngine(v, new Object(), src, "", List.of(
                new SyncEngine.SyncSecret("k", null)), 1, true)) {
            assertEquals("v", v.get("k").asString());
            int initial = src.calls.get();
            // Wait for at least one scheduled pass (interval 1s).
            long deadline = System.currentTimeMillis() + 5000;
            while (src.calls.get() <= initial && System.currentTimeMillis() < deadline) {
                Thread.sleep(50);
            }
            assertTrue(src.calls.get() > initial, "scheduled refresh did not run");
            assertFalse(engine.stats().lastSuccessMs() == null);
        }
    }
}
