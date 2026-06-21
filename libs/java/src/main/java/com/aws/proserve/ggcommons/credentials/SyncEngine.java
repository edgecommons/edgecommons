package com.aws.proserve.ggcommons.credentials;

import java.util.List;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;

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

    private final LocalVault vault;
    private final Object lock;
    private final CentralVaultSource source;
    private final String namespace;
    private final List<SyncSecret> secrets;
    private final ScheduledExecutorService exec;

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
                Thread t = new Thread(r, "ggcommons-cred-sync");
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
        for (SyncSecret s : secrets) {
            String localKey = localKey(s.name());
            // Central id defaults to the namespaced path (per-device); `from` overrides to shared.
            String centralId = s.from() != null ? s.from() : localKey;
            Optional<CentralSecret> cs;
            try {
                cs = source.fetch(centralId);
            } catch (RuntimeException e) {
                LOGGER.warn("central fetch failed for '{}'; using cached value: {}", centralId, e.getMessage());
                continue;
            }
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
                LOGGER.info("secret '{}' synced from central ({})", localKey, centralId);
            }
        }
    }

    @Override
    public void close() {
        if (exec != null) {
            exec.shutdownNow();
        }
    }
}
