package com.mbreissi.edgecommons.parameters;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.TreeMap;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.locks.ReadWriteLock;
import java.util.concurrent.locks.ReentrantReadWriteLock;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import com.mbreissi.edgecommons.credentials.LocalVault;
import com.mbreissi.edgecommons.credentials.PutOptions;
import com.mbreissi.edgecommons.credentials.Secret;
import com.mbreissi.edgecommons.credentials.SecretMeta;

/**
 * Default {@link ParameterService}: a {@link ParameterSource} behind an offline-first cache,
 * optionally refreshed by a background daemon thread. Reads serve from the cache (never the network);
 * a failed refresh keeps serving last-known values. Mirrors the Rust {@code DefaultParameterService}.
 *
 * <p>The cache is <b>source-aware</b>: a remote source (SSM, …) uses a persistent <b>encrypted</b>
 * cache reusing the credentials {@link LocalVault} (the same normative on-disk format) so values
 * survive restarts/offline; an already-local source ({@code mountedDir}, {@code env}) uses an
 * in-memory cache. {@link #close()} stops the background refresh thread.
 */
public final class DefaultParameterService implements ParameterService, AutoCloseable {
    private static final Logger LOGGER = LogManager.getLogger(DefaultParameterService.class);

    static final String SECURE_LABEL = "secure";
    static final String VERSION_LABEL = "pversion";

    /** A cached parameter value (decrypted, in memory). {@code secure} values must not be logged. */
    static final class Cached {
        final byte[] value;
        final boolean secure;
        final String version; // nullable

        Cached(byte[] value, boolean secure, String version) {
            this.value = value;
            this.secure = secure;
            this.version = version;
        }
    }

    /** The cache layer behind the service (offline-first read store). Mirrors the Rust {@code ParamCache} trait. */
    interface ParamCache {
        Optional<Cached> get(String name);

        void put(String name, Cached c);

        List<Map.Entry<String, Cached>> entries(String prefix);

        int size();
    }

    /** In-memory cache for already-local sources ({@code mountedDir}, {@code env}). */
    static final class MemoryCache implements ParamCache {
        private final TreeMap<String, Cached> map = new TreeMap<>();
        private final ReadWriteLock lock = new ReentrantReadWriteLock();

        @Override
        public Optional<Cached> get(String name) {
            lock.readLock().lock();
            try {
                return Optional.ofNullable(map.get(name));
            } finally {
                lock.readLock().unlock();
            }
        }

        @Override
        public void put(String name, Cached c) {
            lock.writeLock().lock();
            try {
                map.put(name, c);
            } finally {
                lock.writeLock().unlock();
            }
        }

        @Override
        public List<Map.Entry<String, Cached>> entries(String prefix) {
            lock.readLock().lock();
            try {
                List<Map.Entry<String, Cached>> out = new ArrayList<>();
                for (Map.Entry<String, Cached> e : map.entrySet()) {
                    if (e.getKey().startsWith(prefix)) {
                        out.add(Map.entry(e.getKey(), e.getValue()));
                    }
                }
                return out;
            } finally {
                lock.readLock().unlock();
            }
        }

        @Override
        public int size() {
            lock.readLock().lock();
            try {
                return map.size();
            } finally {
                lock.readLock().unlock();
            }
        }
    }

    /**
     * Persistent encrypted cache for remote sources — reuses the credentials {@link LocalVault} (the
     * same normative, cross-language on-disk format). The parameter value is the secret bytes;
     * {@code secure} and the upstream version ride along as labels.
     */
    static final class VaultCache implements ParamCache {
        private final LocalVault vault;
        private final Object lock; // serializes vault access (LocalVault is not internally synchronized)

        VaultCache(LocalVault vault, Object lock) {
            this.vault = vault;
            this.lock = lock;
        }

        private static Cached toCached(Secret s) {
            Map<String, String> labels = s.labels() != null ? s.labels() : Map.of();
            boolean secure = "true".equals(labels.get(SECURE_LABEL));
            return new Cached(s.bytes(), secure, labels.get(VERSION_LABEL));
        }

        @Override
        public Optional<Cached> get(String name) {
            synchronized (lock) {
                vault.reloadIfChanged();
                Secret s = vault.get(name);
                return s == null ? Optional.empty() : Optional.of(toCached(s));
            }
        }

        @Override
        public void put(String name, Cached c) {
            PutOptions opts = new PutOptions();
            opts.source = "parameter";
            Map<String, String> labels = new TreeMap<>();
            labels.put(SECURE_LABEL, Boolean.toString(c.secure));
            if (c.version != null) {
                labels.put(VERSION_LABEL, c.version);
            }
            opts.labels = labels;
            synchronized (lock) {
                vault.reloadIfChanged();
                vault.put(name, c.value, opts);
            }
        }

        @Override
        public List<Map.Entry<String, Cached>> entries(String prefix) {
            synchronized (lock) {
                vault.reloadIfChanged();
                List<Map.Entry<String, Cached>> out = new ArrayList<>();
                for (SecretMeta m : vault.list(prefix)) {
                    Secret s = vault.get(m.name());
                    if (s != null) {
                        out.add(Map.entry(m.name(), toCached(s)));
                    }
                }
                return out;
            }
        }

        @Override
        public int size() {
            synchronized (lock) {
                vault.reloadIfChanged();
                return vault.list("").size();
            }
        }
    }

    private final ParameterSource source;
    private final ParamCache cache;
    private final List<String> syncNames;
    /** Each entry: {@code [path, recursive]}. */
    private final List<Map.Entry<String, Boolean>> syncPaths;

    private volatile Long lastRefreshMs; // null until first fully-successful refresh
    private final AtomicLong failures = new AtomicLong();
    private ScheduledExecutorService refresher; // null when no background refresh

    private DefaultParameterService(ParameterSource source, ParamCache cache,
                                    List<String> syncNames, List<Map.Entry<String, Boolean>> syncPaths) {
        this.source = source;
        this.cache = cache;
        this.syncNames = syncNames;
        this.syncPaths = syncPaths;
    }

    /** Build with an in-memory cache — for already-local sources ({@code mountedDir}, {@code env}). */
    public static DefaultParameterService withMemoryCache(ParameterSource source,
                                                          List<String> syncNames,
                                                          List<Map.Entry<String, Boolean>> syncPaths) {
        return new DefaultParameterService(source, new MemoryCache(), syncNames, syncPaths);
    }

    /** Build with a persistent encrypted cache (the credentials {@link LocalVault}) — for remote sources. */
    public static DefaultParameterService withPersistentCache(ParameterSource source, LocalVault vault, Object lock,
                                                              List<String> syncNames,
                                                              List<Map.Entry<String, Boolean>> syncPaths) {
        return new DefaultParameterService(source, new VaultCache(vault, lock), syncNames, syncPaths);
    }

    /**
     * Start a background refresh thread that re-pulls the declared names/paths every
     * {@code intervalSecs} (0 disables it). The thread is a daemon and is stopped by {@link #close()}.
     */
    public DefaultParameterService withRefresh(long intervalSecs) {
        if (intervalSecs > 0) {
            this.refresher = Executors.newSingleThreadScheduledExecutor(r -> {
                Thread t = new Thread(r, "edgecommons-param-refresh");
                t.setDaemon(true);
                return t;
            });
            this.refresher.scheduleWithFixedDelay(() -> {
                try {
                    refresh();
                } catch (RuntimeException e) {
                    // Background refresh is best-effort; failures are already counted/logged in refresh().
                    LOGGER.debug("background parameter refresh failed: {}", e.getMessage());
                }
            }, intervalSecs, intervalSecs, TimeUnit.SECONDS);
        }
        return this;
    }

    @Override
    public Optional<String> get(String name) {
        Optional<byte[]> b = getBytes(name);
        if (b.isEmpty()) {
            return Optional.empty();
        }
        try {
            return Optional.of(StandardCharsets.UTF_8.newDecoder()
                    .decode(java.nio.ByteBuffer.wrap(b.get())).toString());
        } catch (java.nio.charset.CharacterCodingException e) {
            throw new ParameterException("parameter '" + name + "' is not UTF-8");
        }
    }

    @Override
    public Optional<byte[]> getBytes(String name) {
        return cache.get(name).map(c -> c.value);
    }

    @Override
    public Map<String, String> getByPath(String path) {
        Map<String, String> out = new TreeMap<>();
        for (Map.Entry<String, Cached> e : cache.entries(path)) {
            // Skip non-UTF-8 values (parity with Rust, which only inserts decodable strings).
            byte[] v = e.getValue().value;
            try {
                out.put(e.getKey(), StandardCharsets.UTF_8.newDecoder()
                        .decode(java.nio.ByteBuffer.wrap(v)).toString());
            } catch (java.nio.charset.CharacterCodingException ignored) {
                // not a UTF-8 string parameter
            }
        }
        return out;
    }

    @Override
    public List<String> names(String prefix) {
        List<String> out = new ArrayList<>();
        for (Map.Entry<String, Cached> e : cache.entries(prefix)) {
            out.add(e.getKey());
        }
        return out;
    }

    @Override
    public void refresh() {
        RuntimeException anyErr = null;
        for (String name : syncNames) {
            try {
                Optional<ParamValue> v = source.fetch(name);
                v.ifPresent(pv -> cache.put(name, new Cached(pv.value(), pv.secure(), pv.version().orElse(null))));
            } catch (RuntimeException e) {
                LOGGER.warn("parameter refresh failed for '{}' (keeping cached value): {}", name, e.getMessage());
                anyErr = e;
            }
        }
        for (Map.Entry<String, Boolean> entry : syncPaths) {
            String path = entry.getKey();
            boolean recursive = entry.getValue();
            try {
                for (Map.Entry<String, ParamValue> item : source.fetchByPath(path, recursive)) {
                    ParamValue pv = item.getValue();
                    cache.put(item.getKey(), new Cached(pv.value(), pv.secure(), pv.version().orElse(null)));
                }
            } catch (RuntimeException e) {
                LOGGER.warn("parameter path refresh failed for '{}' (keeping cached values): {}", path, e.getMessage());
                anyErr = e;
            }
        }
        if (anyErr != null) {
            failures.incrementAndGet();
            // Offline-first: a refresh failure is non-fatal when we already have cached values.
            if (cache.size() == 0) {
                throw anyErr;
            }
        } else {
            lastRefreshMs = System.currentTimeMillis();
        }
    }

    @Override
    public ParameterStats stats() {
        Long last = lastRefreshMs;
        Long ageMs = last == null ? null : Math.max(0, System.currentTimeMillis() - last);
        return new ParameterStats(cache.size(), ageMs, failures.get(), source.sourceId());
    }

    /** Stops the background refresh thread (if any). Idempotent. */
    @Override
    public void close() {
        if (refresher != null) {
            refresher.shutdownNow();
            refresher = null;
        }
    }
}
