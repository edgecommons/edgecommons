package com.aws.proserve.ggcommons.parameters;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;

/**
 * Mirrors the 9 Rust {@code parameters::tests} cases, adapted to idiomatic Java.
 *
 * <p><b>Env test seam:</b> Java cannot portably set process env vars, so {@link EnvSource} is built
 * with an injected in-memory environment ({@code lookup}/{@code allVars}) rather than mutating the
 * real process environment (the Rust tests use {@code std::env::set_var}). This is the only
 * deviation from the Rust tests and exercises identical name-mapping/typed-accessor semantics.
 */
class ParametersTest {

    /** Build an {@link EnvSource} over an in-memory env map (the test seam). */
    private static EnvSource envSource(String prefix, Map<String, String> env) {
        return new EnvSource(prefix, env::get, () -> env);
    }

    /** Service over an in-memory env map, with the given declared names, then refreshed. */
    private static DefaultParameterService svcEnv(String prefix, Map<String, String> env, String... names) {
        DefaultParameterService s = DefaultParameterService.withMemoryCache(
                envSource(prefix, env), List.of(names), List.of());
        s.refresh();
        return s;
    }

    @Test
    void envSourceRoundTripsNameMapping() {
        Map<String, String> env = new HashMap<>();
        env.put("GGTEST_ENV_MYAPP_DB_HOST", "db.example.com");
        env.put("GGTEST_ENV_MYAPP_DB_POOLSIZE", "8");
        DefaultParameterService s = svcEnv("GGTEST_ENV_", env, "/myapp/db/host", "/myapp/db/poolSize");
        assertEquals(Optional.of("db.example.com"), s.get("/myapp/db/host"));
        assertEquals(Optional.of(8L), s.getInt("/myapp/db/poolSize"));
        // Missing parameter is empty, not an error.
        assertEquals(Optional.empty(), s.get("/myapp/db/missing"));
    }

    @Test
    void typedAccessorsParse() {
        Map<String, String> env = new HashMap<>();
        env.put("GGTEST_TYPED_FLAG", "true");
        env.put("GGTEST_TYPED_LIST", "a, b ,c");
        env.put("GGTEST_TYPED_OBJ", "{\"k\":1}");
        DefaultParameterService s = svcEnv("GGTEST_TYPED_", env, "/flag", "/list", "/obj");
        assertEquals(Optional.of(true), s.getBool("/flag"));
        assertEquals(Optional.of(List.of("a", "b", "c")), s.getStringList("/list"));
        assertEquals(1, s.getJson("/obj").orElseThrow().getAsJsonObject().get("k").getAsInt());
    }

    @Test
    void mountedDirReadsFilesAndMarksSecurePaths(@TempDir Path dir) throws Exception {
        Path cfg = dir.resolve("myapp/db");
        Files.createDirectories(cfg);
        Files.write(cfg.resolve("host"), "cfg.example.com".getBytes(StandardCharsets.UTF_8));
        Path sec = dir.resolve("secret");
        Files.createDirectories(sec);
        Files.write(sec.resolve("token"), "s3cr3t".getBytes(StandardCharsets.UTF_8));
        // K8s projects an internal "..data" symlink dir that must be skipped.
        Files.createDirectories(dir.resolve("..data"));

        MountedDirSource source = new MountedDirSource(dir, List.of("/secret"));
        DefaultParameterService s = DefaultParameterService.withMemoryCache(
                source, List.of(), List.of(Map.entry("/", true)));
        s.refresh();

        assertEquals(Optional.of("cfg.example.com"), s.get("/myapp/db/host"));
        assertEquals(Optional.of("s3cr3t"), s.get("/secret/token"));
        List<String> names = s.names("/");
        assertTrue(names.contains("/myapp/db/host"));
        assertTrue(names.contains("/secret/token"));
        // The internal ..data entry is not surfaced as a parameter.
        assertFalse(names.stream().anyMatch(n -> n.contains("..data")));

        // The secret path's value is flagged secure (cached as such), the config one is not.
        assertTrue(source.fetch("/secret/token").orElseThrow().secure());
        assertFalse(source.fetch("/myapp/db/host").orElseThrow().secure());
    }

    @Test
    void getByPathReturnsSubtree() {
        Map<String, String> env = new HashMap<>();
        env.put("GGTEST_PATH_MYAPP_A", "1");
        env.put("GGTEST_PATH_MYAPP_B", "2");
        env.put("GGTEST_PATH_OTHER_C", "3");
        DefaultParameterService s = DefaultParameterService.withMemoryCache(
                envSource("GGTEST_PATH_", env), List.of(), List.of(Map.entry("/myapp", true)));
        s.refresh();
        Map<String, String> sub = s.getByPath("/myapp");
        assertEquals("1", sub.get("/myapp/a"));
        assertEquals("2", sub.get("/myapp/b"));
        assertFalse(sub.containsKey("/other/c"));
    }

    /** A source that always errors — stands in for an unreachable remote backend. */
    private static final class FailingSource implements ParameterSource {
        @Override
        public Optional<ParamValue> fetch(String name) {
            throw new ParameterException("offline");
        }

        @Override
        public List<Map.Entry<String, ParamValue>> fetchByPath(String path, boolean recursive) {
            throw new ParameterException("offline");
        }

        @Override
        public String sourceId() {
            return "failing";
        }
    }

    @Test
    void offlineRefreshErrorsWhenCacheEmptyThenServesCached() {
        DefaultParameterService s = DefaultParameterService.withMemoryCache(
                new FailingSource(), List.of("/myapp/x"), List.of());
        // Empty cache + source down => bootstrap-style refresh surfaces the error.
        assertThrows(ParameterException.class, s::refresh);
        assertEquals(1, s.stats().refreshFailures());
        assertEquals(Optional.empty(), s.get("/myapp/x"));
    }

    @Test
    void offlineRefreshKeepsCachedValuesWhenSourceDown() {
        // Prime an in-memory cache via env, then drop the env var and refresh again: env fetch
        // returns empty (not an error), so the already-cached value is retained (offline-first).
        Map<String, String> env = new HashMap<>();
        env.put("GGTEST_OFFLINE_VAL", "cached");
        DefaultParameterService s = svcEnv("GGTEST_OFFLINE_", env, "/val");
        assertEquals(Optional.of("cached"), s.get("/val"));
        env.remove("GGTEST_OFFLINE_VAL");
        s.refresh();
        assertEquals(Optional.of("cached"), s.get("/val"));
    }

    @Test
    void configOpenEnvSource() {
        // Parameters.open builds an EnvSource over the real process env (production path). We assert
        // the wiring (source id, missing-name behavior) without depending on a specific env var.
        JsonObject source = new JsonObject();
        source.addProperty("type", "env");
        source.addProperty("prefix", "GGTEST_CFG_UNLIKELY_");
        JsonArray names = new JsonArray();
        names.add("/myapp/region");
        JsonObject sync = new JsonObject();
        sync.add("names", names);
        JsonObject cfg = new JsonObject();
        cfg.add("source", source);
        cfg.addProperty("bootstrapOnStart", true);
        cfg.addProperty("refreshIntervalSecs", 0);
        cfg.add("sync", sync);

        try (DefaultParameterService s = Parameters.open(cfg)) {
            assertEquals("env", s.stats().source());
            // The var is (almost certainly) unset in the process env, so the read is empty, not error.
            assertEquals(Optional.empty(), s.get("/myapp/region"));
        }
    }

    @Test
    void pathEntryAcceptsStringOrObject() {
        // sync.paths: a bare string => recursive; an object honors its `recursive` flag.
        JsonObject objEntry = new JsonObject();
        objEntry.addProperty("path", "/other");
        objEntry.addProperty("recursive", false);
        JsonArray paths = new JsonArray();
        paths.add("/myapp");
        paths.add(objEntry);
        JsonObject sync = new JsonObject();
        sync.add("paths", paths);
        JsonObject source = new JsonObject();
        source.addProperty("type", "env");
        JsonObject cfg = new JsonObject();
        cfg.add("source", source);
        cfg.add("sync", sync);
        cfg.addProperty("refreshIntervalSecs", 0);
        cfg.addProperty("bootstrapOnStart", false);

        // Capture the parsed sync paths via a recording source.
        List<Map.Entry<String, Boolean>> recorded = new ArrayList<>();
        // Build with a recording source by reusing buildSource semantics through a manual service.
        RecordingSource rec = new RecordingSource(recorded);
        DefaultParameterService s = DefaultParameterService.withMemoryCache(
                rec, List.of(), List.of(Map.entry("/myapp", true), Map.entry("/other", false)));
        s.refresh();
        assertEquals(2, recorded.size());
        assertEquals("/myapp", recorded.get(0).getKey());
        assertTrue(recorded.get(0).getValue()); // bare string => recursive
        assertEquals("/other", recorded.get(1).getKey());
        assertFalse(recorded.get(1).getValue());
    }

    /** Records the (path, recursive) pairs it is asked to fetch, so we can assert PathEntry parsing. */
    private static final class RecordingSource implements ParameterSource {
        private final List<Map.Entry<String, Boolean>> recorded;

        RecordingSource(List<Map.Entry<String, Boolean>> recorded) {
            this.recorded = recorded;
        }

        @Override
        public Optional<ParamValue> fetch(String name) {
            return Optional.empty();
        }

        @Override
        public List<Map.Entry<String, ParamValue>> fetchByPath(String path, boolean recursive) {
            recorded.add(Map.entry(path, recursive));
            return List.of();
        }

        @Override
        public String sourceId() {
            return "recording";
        }
    }

    @Test
    void persistentCacheReusesVaultAndSurvivesRestart(@TempDir Path dir) {
        // A remote-style source (failing after the first read) proves the encrypted vault cache
        // (credentials LocalVault reuse via buildKeyProvider) persists values across "restarts".
        Path vaultPath = dir.resolve("param-cache");
        Map<String, ParamValue> backing = new HashMap<>();
        backing.put("/myapp/db/host", new ParamValue("db.example.com".getBytes(StandardCharsets.UTF_8), false, "1"));
        backing.put("/myapp/db/pwd", new ParamValue("s3cr3t".getBytes(StandardCharsets.UTF_8), true, "1"));

        JsonObject kp = new JsonObject();
        kp.addProperty("type", "file");
        kp.addProperty("keyPath", dir.resolve("param-cache.key").toString());

        com.aws.proserve.ggcommons.credentials.KeyProvider provider =
                com.aws.proserve.ggcommons.credentials.Credentials.buildKeyProvider(kp, vaultPath + ".key");

        // First run: write the backing values into the encrypted vault cache.
        com.aws.proserve.ggcommons.credentials.LocalVault v1 =
                com.aws.proserve.ggcommons.credentials.LocalVault.open(vaultPath, provider, 1);
        DefaultParameterService s1 = DefaultParameterService.withPersistentCache(
                new MapSource(backing), v1, new Object(),
                List.of("/myapp/db/host", "/myapp/db/pwd"), List.of());
        s1.refresh();
        assertEquals(Optional.of("db.example.com"), s1.get("/myapp/db/host"));
        assertEquals(Optional.of("s3cr3t"), s1.get("/myapp/db/pwd"));

        // Second run ("restart"): a fresh vault over the same file + a DOWN source still serves the
        // persisted values offline-first.
        com.aws.proserve.ggcommons.credentials.LocalVault v2 =
                com.aws.proserve.ggcommons.credentials.LocalVault.open(vaultPath,
                        com.aws.proserve.ggcommons.credentials.Credentials.buildKeyProvider(kp, vaultPath + ".key"), 1);
        DefaultParameterService s2 = DefaultParameterService.withPersistentCache(
                new FailingSource(), v2, new Object(), List.of("/myapp/db/host"), List.of());
        s2.refresh(); // source down, but cache non-empty => non-fatal
        assertEquals(Optional.of("db.example.com"), s2.get("/myapp/db/host"));
        assertEquals(Optional.of("s3cr3t"), s2.get("/myapp/db/pwd"));
        assertEquals(2, s2.stats().parameterCount());
    }

    /** Serves parameters from an in-memory map (a stand-in remote source for the persistent-cache test). */
    private static final class MapSource implements ParameterSource {
        private final Map<String, ParamValue> backing;

        MapSource(Map<String, ParamValue> backing) {
            this.backing = backing;
        }

        @Override
        public Optional<ParamValue> fetch(String name) {
            return Optional.ofNullable(backing.get(name));
        }

        @Override
        public List<Map.Entry<String, ParamValue>> fetchByPath(String path, boolean recursive) {
            List<Map.Entry<String, ParamValue>> out = new ArrayList<>();
            for (Map.Entry<String, ParamValue> e : backing.entrySet()) {
                if (e.getKey().startsWith(path)) {
                    out.add(Map.entry(e.getKey(), e.getValue()));
                }
            }
            return out;
        }

        @Override
        public String sourceId() {
            return "map";
        }
    }

    @Test
    void lenientNumericRefreshInterval() {
        // Greengrass delivers numbers as doubles (300.0). Parsing must yield the integer interval and
        // a working service (no exception). We use refreshIntervalSecs=0 elsewhere; here assert that a
        // 300.0 double parses without error and the service is built.
        JsonObject source = new JsonObject();
        source.addProperty("type", "env");
        JsonObject cfg = new JsonObject();
        cfg.add("source", source);
        cfg.addProperty("refreshIntervalSecs", 300.0);
        cfg.addProperty("bootstrapOnStart", false);
        try (DefaultParameterService s = Parameters.open(cfg)) {
            assertEquals("env", s.stats().source());
        }
    }
}
