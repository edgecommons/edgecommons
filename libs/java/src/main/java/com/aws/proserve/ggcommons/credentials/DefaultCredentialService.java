package com.aws.proserve.ggcommons.credentials;

import java.util.List;
import java.util.Optional;

/**
 * The default {@link CredentialService}: a {@link LocalVault} guarded by a lock. Each read first
 * picks up any cross-process change (the shared device vault may be written by another component).
 */
public final class DefaultCredentialService implements CredentialService {
    private final LocalVault vault;
    private final Object lock = new Object();

    public DefaultCredentialService(LocalVault vault) {
        this.vault = vault;
    }

    @Override
    public Optional<Secret> get(String name) {
        synchronized (lock) {
            vault.reloadIfChanged();
            return Optional.ofNullable(vault.get(name));
        }
    }

    @Override
    public Optional<Secret> getVersion(String name, String version) {
        synchronized (lock) {
            vault.reloadIfChanged();
            return Optional.ofNullable(vault.getVersion(name, version));
        }
    }

    @Override
    public boolean exists(String name) {
        synchronized (lock) {
            vault.reloadIfChanged();
            return vault.exists(name);
        }
    }

    @Override
    public List<SecretMeta> list(String prefix) {
        synchronized (lock) {
            vault.reloadIfChanged();
            return vault.list(prefix);
        }
    }

    @Override
    public List<String> versions(String name) {
        synchronized (lock) {
            vault.reloadIfChanged();
            return vault.versions(name);
        }
    }

    @Override
    public String put(String name, byte[] value, PutOptions opts) {
        synchronized (lock) {
            vault.reloadIfChanged();
            return vault.put(name, value, opts);
        }
    }

    @Override
    public boolean delete(String name) {
        synchronized (lock) {
            vault.reloadIfChanged();
            return vault.delete(name);
        }
    }
}
