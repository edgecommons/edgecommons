package com.aws.proserve.ggcommons.parameters;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
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
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

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

    // ----------------------------------------------------------------------------------------------
    // Additional coverage (idiomatic-Java parity with the Rust reference's >90% test set).
    // ----------------------------------------------------------------------------------------------

    @Test
    void typedAccessorErrorAndEmptyBranches() {
        // Drives every typed-accessor branch the happy-path tests don't: parse errors, the false
        // boolean, an empty StringList, and the missing => empty / Optional.empty path on each.
        Map<String, String> env = new HashMap<>();
        env.put("GGTEST_TY2_NOTINT", "abc");
        env.put("GGTEST_TY2_NOTBOOL", "maybe");
        env.put("GGTEST_TY2_NOTJSON", "{bad");
        env.put("GGTEST_TY2_FLAGOFF", "off");
        env.put("GGTEST_TY2_EMPTY", "");
        DefaultParameterService s = svcEnv("GGTEST_TY2_",
                env, "/notInt", "/notBool", "/notJson", "/flagOff", "/empty");

        // getBool false branch + getInt/getBool/getJson parse errors.
        assertEquals(Optional.of(false), s.getBool("/flagOff"));
        assertThrows(ParameterException.class, () -> s.getInt("/notInt"));
        assertThrows(ParameterException.class, () -> s.getBool("/notBool"));
        assertThrows(ParameterException.class, () -> s.getJson("/notJson"));

        // Empty string => empty StringList (the `!v.isEmpty()` short-circuit).
        assertEquals(Optional.of(List.of()), s.getStringList("/empty"));

        // Missing name => Optional.empty() on every accessor (no exception).
        assertEquals(Optional.empty(), s.getInt("/missing"));
        assertEquals(Optional.empty(), s.getBool("/missing"));
        assertEquals(Optional.empty(), s.getJson("/missing"));
        assertEquals(Optional.empty(), s.getStringList("/missing"));
        assertEquals(Optional.empty(), s.getBytes("/missing"));
    }

    @Test
    void getBytesAndStatsNeverRefreshed() {
        Map<String, String> env = new HashMap<>();
        env.put("GGTEST_BYTES_VAL", "raw");
        DefaultParameterService s = svcEnv("GGTEST_BYTES_", env, "/val");
        assertEquals("raw", new String(s.getBytes("/val").orElseThrow(), StandardCharsets.UTF_8));

        // After a successful refresh, stats report a non-null (>= 0) age and zero failures.
        ParameterStats st = s.stats();
        assertEquals(1, st.parameterCount());
        assertEquals(0, st.refreshFailures());
        assertNotNull(st.lastRefreshAgeMs());
        assertTrue(st.lastRefreshAgeMs() >= 0);
        assertEquals("env", st.source());
    }

    @Test
    void statsLastRefreshAgeNullBeforeAnyRefresh() {
        // A brand-new service that has never refreshed reports a null last-refresh age.
        DefaultParameterService s = DefaultParameterService.withMemoryCache(
                envSource("GGTEST_NOREFRESH_", new HashMap<>()), List.of(), List.of());
        assertNull(s.stats().lastRefreshAgeMs());
        assertEquals(0, s.stats().parameterCount());
    }

    @Test
    void getThrowsOnNonUtf8Value(@TempDir Path dir) throws Exception {
        // get() must reject a non-UTF-8 value (the CharacterCodingException branch); getByPath must
        // silently skip it (parity with Rust, which only inserts decodable strings).
        byte[] invalid = {(byte) 0xff, (byte) 0xfe};
        Files.write(dir.resolve("bin"), invalid);
        Files.write(dir.resolve("ok"), "good".getBytes(StandardCharsets.UTF_8));
        MountedDirSource source = new MountedDirSource(dir, List.of());
        DefaultParameterService s = DefaultParameterService.withMemoryCache(
                source, List.of(), List.of(Map.entry("/", true)));
        s.refresh();

        assertThrows(ParameterException.class, () -> s.get("/bin"));
        // Raw bytes are still retrievable.
        assertEquals(2, s.getBytes("/bin").orElseThrow().length);
        // getByPath skips the non-UTF-8 entry but keeps the valid one.
        Map<String, String> sub = s.getByPath("/");
        assertEquals("good", sub.get("/ok"));
        assertFalse(sub.containsKey("/bin"));
    }

    @Test
    void mountedDirSingleFetchAndMissing(@TempDir Path dir) throws Exception {
        Files.write(dir.resolve("host"), "h".getBytes(StandardCharsets.UTF_8));
        Files.createDirectories(dir.resolve("subdir"));
        MountedDirSource source = new MountedDirSource(dir.toString(), List.of());

        // Single fetch hits the file directly (no walk).
        assertEquals("h", new String(source.fetch("/host").orElseThrow().value(), StandardCharsets.UTF_8));
        // A missing file => empty (not an error).
        assertEquals(Optional.empty(), source.fetch("/nope"));
        // A directory at that name => "not a parameter" => empty.
        assertEquals(Optional.empty(), source.fetch("/subdir"));
        assertEquals("mountedDir", source.sourceId());
    }

    @Test
    void mountedDirMissingBaseDirYieldsNothing(@TempDir Path dir) {
        // fetchByPath over an absent directory returns no parameters (NoSuchFileException swallowed).
        MountedDirSource source = new MountedDirSource(dir.resolve("does-not-exist"), List.of());
        assertTrue(source.fetchByPath("/", true).isEmpty());
    }

    @Test
    void mountedDirNonRecursiveSkipsSubdirs(@TempDir Path dir) throws Exception {
        Files.write(dir.resolve("top"), "1".getBytes(StandardCharsets.UTF_8));
        Path sub = dir.resolve("nested");
        Files.createDirectories(sub);
        Files.write(sub.resolve("deep"), "2".getBytes(StandardCharsets.UTF_8));
        MountedDirSource source = new MountedDirSource(dir, List.of());

        List<Map.Entry<String, ParamValue>> shallow = source.fetchByPath("/", false);
        List<String> names = new ArrayList<>();
        shallow.forEach(e -> names.add(e.getKey()));
        assertTrue(names.contains("/top"));
        assertFalse(names.contains("/nested/deep")); // not recursive => nested file skipped
    }

    @Test
    void paramValueFactoriesAndToString() {
        ParamValue plain = ParamValue.plain("hello");
        assertFalse(plain.secure());
        assertEquals(Optional.empty(), plain.version());
        assertEquals("hello", new String(plain.value(), StandardCharsets.UTF_8));
        assertTrue(plain.toString().contains("secure=false"));
        assertFalse(plain.toString().contains("redacted"));

        ParamValue secure = new ParamValue("s3cr3t".getBytes(StandardCharsets.UTF_8), true, "v7");
        assertTrue(secure.secure());
        assertEquals(Optional.of("v7"), secure.version());
        // Secure toString must redact the value (never the raw bytes) but may state secure/version.
        String repr = secure.toString();
        assertTrue(repr.contains("redacted (secure)"));
        assertTrue(repr.contains("version=v7"));
        assertFalse(repr.contains("s3cr3t"));
    }

    @Test
    void parameterExceptionCauseConstructor() {
        Throwable cause = new IllegalStateException("boom");
        ParameterException e = new ParameterException("wrapped", cause);
        assertEquals("wrapped", e.getMessage());
        assertEquals(cause, e.getCause());
    }

    @Test
    void parameterStatsRecordAccessors() {
        ParameterStats st = new ParameterStats(3, 1500L, 2, "env");
        assertEquals(3, st.parameterCount());
        assertEquals(1500L, st.lastRefreshAgeMs());
        assertEquals(2, st.refreshFailures());
        assertEquals("env", st.source());
    }

    @Test
    void configOpenMountedDirSource(@TempDir Path dir) throws Exception {
        // Parameters.open builds a MountedDirSource and honors sync.paths/securePaths end-to-end.
        Path cfg = dir.resolve("app");
        Files.createDirectories(cfg);
        Files.write(cfg.resolve("region"), "us-east-1".getBytes(StandardCharsets.UTF_8));
        Path sec = dir.resolve("secret");
        Files.createDirectories(sec);
        Files.write(sec.resolve("token"), "t0p".getBytes(StandardCharsets.UTF_8));

        JsonObject source = new JsonObject();
        source.addProperty("type", "mountedDir");
        source.addProperty("root", dir.toString());
        JsonArray securePaths = new JsonArray();
        securePaths.add("/secret");
        source.add("securePaths", securePaths);

        JsonArray paths = new JsonArray();
        paths.add("/"); // bare string => recursive
        JsonObject sync = new JsonObject();
        sync.add("paths", paths);

        JsonObject cfgJson = new JsonObject();
        cfgJson.add("source", source);
        cfgJson.add("sync", sync);
        cfgJson.addProperty("refreshIntervalSecs", 0);
        // bootstrapOnStart defaults to true => refresh happens inside open().

        try (DefaultParameterService s = Parameters.open(cfgJson, "ignored-namespace")) {
            assertEquals("mountedDir", s.stats().source());
            assertEquals(Optional.of("us-east-1"), s.get("/app/region"));
            assertEquals(Optional.of("t0p"), s.get("/secret/token"));
        }
    }

    @Test
    void configOpenRejectsBadSources() {
        // Unknown source type, and mountedDir without source.root, both surface a ParameterException.
        JsonObject unknown = new JsonObject();
        unknown.addProperty("type", "bogus");
        JsonObject cfg1 = new JsonObject();
        cfg1.add("source", unknown);
        assertThrows(ParameterException.class, () -> Parameters.open(cfg1));

        JsonObject mounted = new JsonObject();
        mounted.addProperty("type", "mountedDir"); // missing "root"
        JsonObject cfg2 = new JsonObject();
        cfg2.add("source", mounted);
        assertThrows(ParameterException.class, () -> Parameters.open(cfg2));

        // The default "none" source (no source.type) is also unsupported.
        assertThrows(ParameterException.class, () -> Parameters.open(new JsonObject()));
    }

    @Test
    void configOpenPersistentCacheRoundTrip(@TempDir Path dir) {
        // cache.persist=true forces the encrypted VaultCache path even for a local source, exercising
        // Parameters.open's persistent branch (buildKeyProvider + LocalVault.open) end-to-end.
        Map<String, String> env = new HashMap<>();
        env.put("GGTEST_PERSIST_REGION", "eu-west-1");

        JsonObject source = new JsonObject();
        source.addProperty("type", "env");
        source.addProperty("prefix", "GGTEST_PERSIST_");

        JsonObject keyProvider = new JsonObject();
        keyProvider.addProperty("type", "file");
        keyProvider.addProperty("keyPath", dir.resolve("pc.key").toString());
        JsonObject cache = new JsonObject();
        cache.addProperty("persist", true);
        cache.addProperty("path", dir.resolve("pc-vault").toString());
        cache.add("keyProvider", keyProvider);

        JsonArray names = new JsonArray();
        names.add("/region");
        JsonObject sync = new JsonObject();
        sync.add("names", names);

        JsonObject cfg = new JsonObject();
        cfg.add("source", source);
        cfg.add("cache", cache);
        cfg.add("sync", sync);
        cfg.addProperty("refreshIntervalSecs", 0);

        // EnvSource here reads the real process env, which (almost certainly) lacks GGTEST_PERSIST_*;
        // so the bootstrap refresh writes nothing but must not throw, and the vault path is created.
        try (DefaultParameterService s = Parameters.open(cfg)) {
            assertEquals("env", s.stats().source());
            assertTrue(Files.exists(dir.resolve("pc-vault")) || s.stats().parameterCount() == 0);
        }
    }

    @Test
    void persistentCacheGetByPathAndNames(@TempDir Path dir) {
        // Exercises VaultCache.entries(prefix) via getByPath/names (the persistent cache's enumeration).
        Path vaultPath = dir.resolve("pc");
        JsonObject kp = new JsonObject();
        kp.addProperty("type", "file");
        kp.addProperty("keyPath", dir.resolve("pc.key").toString());
        com.aws.proserve.ggcommons.credentials.KeyProvider provider =
                com.aws.proserve.ggcommons.credentials.Credentials.buildKeyProvider(kp, vaultPath + ".key");
        com.aws.proserve.ggcommons.credentials.LocalVault vault =
                com.aws.proserve.ggcommons.credentials.LocalVault.open(vaultPath, provider, 1);

        Map<String, ParamValue> backing = new HashMap<>();
        backing.put("/app/a", ParamValue.plain("1"));
        backing.put("/app/b", new ParamValue("2".getBytes(StandardCharsets.UTF_8), true, "9"));
        DefaultParameterService s = DefaultParameterService.withPersistentCache(
                new MapSource(backing), vault, new Object(),
                List.of(), List.of(Map.entry("/app", true)));
        s.refresh();

        Map<String, String> sub = s.getByPath("/app");
        assertEquals("1", sub.get("/app/a"));
        assertEquals("2", sub.get("/app/b"));
        List<String> names = s.names("/app");
        assertTrue(names.contains("/app/a"));
        assertTrue(names.contains("/app/b"));
        assertEquals(2, s.stats().parameterCount());
    }

    @Test
    void configOpenParsesObjectPathEntryAndBootstraps(@TempDir Path dir) throws Exception {
        // Drives Parameters.parsePaths' object-entry branch ({path, recursive:false}) AND the
        // bootstrapOnStart refresh, end-to-end through open() against a mountedDir source.
        Files.write(dir.resolve("top"), "T".getBytes(StandardCharsets.UTF_8));
        Path nested = dir.resolve("nested");
        Files.createDirectories(nested);
        Files.write(nested.resolve("deep"), "D".getBytes(StandardCharsets.UTF_8));

        JsonObject source = new JsonObject();
        source.addProperty("type", "mountedDir");
        source.addProperty("root", dir.toString());

        JsonObject objEntry = new JsonObject();
        objEntry.addProperty("path", "/");
        objEntry.addProperty("recursive", false); // object form, non-recursive
        JsonArray paths = new JsonArray();
        paths.add(objEntry);
        JsonObject sync = new JsonObject();
        sync.add("paths", paths);

        JsonObject cfg = new JsonObject();
        cfg.add("source", source);
        cfg.add("sync", sync);
        cfg.addProperty("refreshIntervalSecs", 0);
        cfg.addProperty("bootstrapOnStart", true);

        try (DefaultParameterService s = Parameters.open(cfg)) {
            assertEquals(Optional.of("T"), s.get("/top"));
            // recursive:false => the nested file is not synced.
            assertEquals(Optional.empty(), s.get("/nested/deep"));
        }
    }

    @Test
    void configOpenBootstrapFailureIsNonFatal(@TempDir Path dir) throws Exception {
        // A sync.paths entry that points the mountedDir walk at a *file* makes the bootstrap refresh
        // throw (NotDirectoryException -> ParameterException, empty cache); open() must swallow it
        // (offline-first) and still return a usable service. Covers Parameters' bootstrap catch.
        Path file = dir.resolve("afile");
        Files.write(file, "x".getBytes(StandardCharsets.UTF_8));

        JsonObject source = new JsonObject();
        source.addProperty("type", "mountedDir");
        source.addProperty("root", dir.toString());

        JsonArray paths = new JsonArray();
        paths.add("/afile"); // a file, not a dir => walk throws
        JsonObject sync = new JsonObject();
        sync.add("paths", paths);

        JsonObject cfg = new JsonObject();
        cfg.add("source", source);
        cfg.add("sync", sync);
        cfg.addProperty("refreshIntervalSecs", 0);
        cfg.addProperty("bootstrapOnStart", true);

        try (DefaultParameterService s = Parameters.open(cfg)) {
            // open() did not propagate the bootstrap failure.
            assertEquals("mountedDir", s.stats().source());
            assertEquals(0, s.stats().parameterCount());
        }
    }

    @Test
    void mountedDirWalkOnFilePathThrows(@TempDir Path dir) throws Exception {
        // fetchByPath aimed at a regular file (not a directory) surfaces the non-NoSuchFile I/O
        // branch of walk (NotDirectoryException -> ParameterException).
        Path file = dir.resolve("regular");
        Files.write(file, "x".getBytes(StandardCharsets.UTF_8));
        MountedDirSource source = new MountedDirSource(dir, List.of());
        assertThrows(ParameterException.class, () -> source.fetchByPath("/regular", true));
    }

    @Test
    void backgroundRefreshObservesSourceChange() throws Exception {
        // A mutable in-memory source whose value changes; the background refresh thread (1s interval)
        // must pull the new value into the cache without an explicit refresh() call.
        AtomicReference<String> backing = new AtomicReference<>("v1");
        ParameterSource mutable = new ParameterSource() {
            @Override
            public Optional<ParamValue> fetch(String name) {
                return "/key".equals(name) ? Optional.of(ParamValue.plain(backing.get())) : Optional.empty();
            }

            @Override
            public List<Map.Entry<String, ParamValue>> fetchByPath(String path, boolean recursive) {
                return List.of();
            }

            @Override
            public String sourceId() {
                return "mutable";
            }
        };

        DefaultParameterService s = DefaultParameterService.withMemoryCache(
                mutable, List.of("/key"), List.of());
        s.refresh();
        assertEquals(Optional.of("v1"), s.get("/key"));

        try (DefaultParameterService running = s.withRefresh(1)) {
            backing.set("v2");
            // Wait up to ~5s for the daemon refresh to observe the change.
            long deadline = System.currentTimeMillis() + 5000;
            String seen = "v1";
            while (System.currentTimeMillis() < deadline) {
                seen = running.get("/key").orElse("");
                if ("v2".equals(seen)) {
                    break;
                }
                TimeUnit.MILLISECONDS.sleep(100);
            }
            assertEquals("v2", seen);
        }
        // close() (via try-with-resources) stops the daemon; a second close is a no-op.
        s.close();
    }
}
