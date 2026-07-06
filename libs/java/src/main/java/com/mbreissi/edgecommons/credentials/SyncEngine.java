package com.mbreissi.edgecommons.credentials;

import java.util.List;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

/**
 * Seeds + refreshes the local vault from a {@link CentralVaultSource} — offline-first, selective,
 * rotation-aware. Bootstrap is synchronous; refresh runs on a daemon scheduler. {@link #close()}
 * stops it.
 */
public final class SyncEngine implements AutoCloseable {
    private static final Logger LOGGER = LogManager.getLogger(SyncEngine.class);

    /** One secret to sync: caller-facing {@code name} and an optional central-id override. */
    public record SyncSecret(String name, String from) {
    }

    /**
     * A snapshot of the sync engine's observability counters (read by the credential metrics bridge).
     *
     * @param lastSuccessMs epoch-ms of the last fully-successful pass, or {@code null} if never
     * @param failures      total central-fetch failures
     * @param rotations     total synced secrets actually written (rotations)
     */
    public record SyncStats(Long lastSuccessMs, long failures, long rotations) {
    }

    private final LocalVault vault;
    private final Object lock;
    private final CentralVaultSource source;
    private final String namespace;
    private final List<SyncSecret> secrets;
    private final ScheduledExecutorService exec;

    // Observability counters (read by the credential metrics bridge).
    private volatile long lastSuccessMs = -1;
    private final AtomicLong failures = new AtomicLong();
    private final AtomicLong rotations = new AtomicLong();

    public SyncEngine(LocalVault vault, Object lock, CentralVaultSource source, String namespace,
                      List<SyncSecret> secrets, long intervalSecs, boolean bootstrap) {
        this.vault = vault;
        this.lock = lock;
        this.source = source;
        this.namespace = namespace;
        this.secrets = secrets;
        if (bootstrap) {
            syncNow();
        }
        if (intervalSecs > 0) {
            this.exec = Executors.newSingleThreadScheduledExecutor(r -> {
                Thread t = new Thread(r, "edgecommons-cred-sync");
                t.setDaemon(true);
                return t;
            });
            this.exec.scheduleWithFixedDelay(this::syncNow, intervalSecs, intervalSecs, TimeUnit.SECONDS);
        } else {
            this.exec = null;
        }
    }

    private String localKey(String name) {
        return namespace.isEmpty() ? name : namespace + "/" + name;
    }

    /** Force an immediate sync pass. */
    public void syncNow() {
        boolean anySuccess = false;
        for (SyncSecret s : secrets) {
            String localKey = localKey(s.name());
            // Central id defaults to the namespaced path (per-device); `from` overrides to shared.
            String centralId = s.from() != null ? s.from() : localKey;
            Optional<CentralSecret> cs;
            try {
                cs = source.fetch(centralId);
            } catch (RuntimeException e) {
                // Offline-first: keep the cached value, surface the staleness.
                failures.incrementAndGet();
                LOGGER.warn("central fetch failed for '{}'; using cached value: {}", centralId, e.getMessage());
                continue;
            }
            anySuccess = true;
            if (cs.isEmpty()) {
                continue;
            }
            synchronized (lock) {
                vault.reloadIfChanged();
                if (Objects.equals(vault.latestCentralVersionId(localKey), cs.get().centralVersionId())) {
                    continue;
                }
                PutOptions opts = new PutOptions();
                opts.source = "central";
                opts.centralVersionId = cs.get().centralVersionId();
                opts.labels = cs.get().labels();
                vault.put(localKey, cs.get().bytes(), opts);
                rotations.incrementAndGet();
                LOGGER.info("secret '{}' synced from central ({})", localKey, centralId);
            }
        }
        if (anySuccess) {
            lastSuccessMs = System.currentTimeMillis();
        }
    }

    /** A snapshot of the sync counters (for the credential metrics bridge). */
    public SyncStats stats() {
        return new SyncStats(lastSuccessMs < 0 ? null : lastSuccessMs, failures.get(), rotations.get());
    }

    @Override
    public void close() {
        if (exec != null) {
            exec.shutdownNow();
        }
    }
}
