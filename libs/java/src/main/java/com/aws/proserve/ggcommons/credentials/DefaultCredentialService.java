package com.aws.proserve.ggcommons.credentials;

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

    public DefaultCredentialService(LocalVault vault) {
        this(vault, "", new Object(), null);
    }

    public DefaultCredentialService(LocalVault vault, String namespace, Object lock, SyncEngine sync) {
        this.vault = vault;
        this.namespace = namespace == null ? "" : namespace;
        this.lock = lock;
        this.sync = sync;
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
        synchronized (lock) {
            vault.reloadIfChanged();
            Secret s = vault.get(full(name));
            return s == null ? Optional.empty() : Optional.of(relName(s));
        }
    }

    @Override
    public Optional<Secret> getVersion(String name, String version) {
        synchronized (lock) {
            vault.reloadIfChanged();
            Secret s = vault.getVersion(full(name), version);
            return s == null ? Optional.empty() : Optional.of(relName(s));
        }
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
        synchronized (lock) {
            vault.reloadIfChanged();
            return vault.put(full(name), value, opts);
        }
    }

    @Override
    public boolean delete(String name) {
        synchronized (lock) {
            vault.reloadIfChanged();
            return vault.delete(full(name));
        }
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
