package com.mbreissi.ggcommons.credentials;

import java.util.List;
import java.util.Optional;
import java.util.stream.Collectors;

/**
 * The default {@link CredentialService}: a {@link LocalVault} guarded by a lock. Each read first
 * picks up any cross-process change (the shared device vault may be written by another component).
 *
 * <p>{@code namespace} (<thingName>/<componentName>) is prepended transparently to every key and
 * stripped from returned names, so a shared device vault can't collide across components.
 */
public final class DefaultCredentialService implements CredentialService {
    private final LocalVault vault;
    private final Object lock;
    private final String namespace;
    private final SyncEngine sync;

    /**
     * Audit sink for access events ({@code null} = auditing off). Set via {@link #withAudit(AuditSink)};
     * the config path enables it ({@code credentials.audit.enabled}) with the default logging sink.
     */
    private AuditSink audit;

    public DefaultCredentialService(LocalVault vault) {
        this(vault, "", new Object(), null);
    }

    public DefaultCredentialService(LocalVault vault, String namespace, Object lock, SyncEngine sync) {
        this.vault = vault;
        this.namespace = namespace == null ? "" : namespace;
        this.lock = lock;
        this.sync = sync;
    }

    /** Attach (or clear) the audit sink — access events are emitted to it. Fluent; returns {@code this}. */
    public DefaultCredentialService withAudit(AuditSink sink) {
        this.audit = sink;
        return this;
    }

    /** Emit an audit event if an audit sink is configured (no-op otherwise). Never includes the value. */
    private void audit(String op, String name, String version, String source, String outcome) {
        AuditSink sink = this.audit;
        if (sink != null) {
            sink.record(new AuditEvent(op, name, version, source, outcome));
        }
    }

    private String full(String name) {
        return namespace.isEmpty() ? name : namespace + "/" + name;
    }

    private String rel(String full) {
        String prefix = namespace + "/";
        return (!namespace.isEmpty() && full.startsWith(prefix)) ? full.substring(prefix.length()) : full;
    }

    private Secret relName(Secret s) {
        return new Secret(rel(s.name()), s.version(), s.bytes(), s.labels(), s.createdMs(), s.source(), s.contentType());
    }

    @Override
    public Optional<Secret> get(String name) {
        Secret result;
        synchronized (lock) {
            vault.reloadIfChanged();
            Secret s = vault.get(full(name));
            result = s == null ? null : relName(s);
        }
        if (result != null) {
            audit("get", name, result.version(), result.source(), "hit");
        } else {
            audit("get", name, "-", "-", "miss");
        }
        return Optional.ofNullable(result);
    }

    @Override
    public Optional<Secret> getVersion(String name, String version) {
        Secret result;
        synchronized (lock) {
            vault.reloadIfChanged();
            Secret s = vault.getVersion(full(name), version);
            result = s == null ? null : relName(s);
        }
        if (result != null) {
            audit("get", name, result.version(), result.source(), "hit");
        } else {
            audit("get", name, version, "-", "miss");
        }
        return Optional.ofNullable(result);
    }

    @Override
    public boolean exists(String name) {
        synchronized (lock) {
            vault.reloadIfChanged();
            return vault.exists(full(name));
        }
    }

    @Override
    public List<SecretMeta> list(String prefix) {
        synchronized (lock) {
            vault.reloadIfChanged();
            return vault.list(full(prefix)).stream()
                    .map(m -> new SecretMeta(rel(m.name()), m.version(), m.createdMs(), m.ttlSecs(), m.source(), m.labels()))
                    .collect(Collectors.toList());
        }
    }

    @Override
    public List<String> versions(String name) {
        synchronized (lock) {
            vault.reloadIfChanged();
            return vault.versions(full(name));
        }
    }

    @Override
    public String put(String name, byte[] value, PutOptions opts) {
        String version;
        synchronized (lock) {
            vault.reloadIfChanged();
            version = vault.put(full(name), value, opts);
        }
        audit("put", name, version, "local", "ok");
        return version;
    }

    @Override
    public boolean delete(String name) {
        boolean deleted;
        synchronized (lock) {
            vault.reloadIfChanged();
            deleted = vault.delete(full(name));
        }
        audit("delete", name, "-", "-", deleted ? "ok" : "miss");
        return deleted;
    }

    @Override
    public void refresh() {
        if (sync != null) {
            sync.syncNow();
        }
    }

    @Override
    public CredentialStats stats() {
        long secretCount = list("").size();
        if (sync == null) {
            return new CredentialStats(secretCount, null, 0, 0);
        }
        SyncEngine.SyncStats s = sync.stats();
        Long ageMs = s.lastSuccessMs() == null ? null : Math.max(0, System.currentTimeMillis() - s.lastSuccessMs());
        return new CredentialStats(secretCount, ageMs, s.failures(), s.rotations());
    }
}
