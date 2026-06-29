package com.mbreissi.ggcommons.credentials;

import com.mbreissi.ggcommons.credentials.VaultModel.KekInfo;

/**
 * KEK custodian: wraps/unwraps the vault DEK without exposing the KEK. Phase 1 ships
 * {@link FileKeyProvider}; {@code kms}/{@code greengrass}/{@code pkcs11} slot in behind this same
 * interface without a format change.
 */
public interface KeyProvider {
    /** Custodian id written to {@code KekInfo.provider} (e.g. {@code "file"}). */
    String providerId();

    /** Wrap {@code dek} for {@code vaultId}, producing the {@link KekInfo} persisted in the file. */
    KekInfo wrapDek(String vaultId, byte[] dek);

    /** Unwrap the DEK described by {@code kek} for {@code vaultId}. */
    byte[] unwrapDek(String vaultId, KekInfo kek);
}
