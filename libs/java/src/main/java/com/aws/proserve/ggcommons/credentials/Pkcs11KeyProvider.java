package com.aws.proserve.ggcommons.credentials;

import java.security.Key;
import java.security.KeyStore;
import java.security.Provider;
import java.security.Security;
import java.util.Base64;

import javax.crypto.Cipher;
import javax.crypto.spec.GCMParameterSpec;

import com.aws.proserve.ggcommons.credentials.VaultModel.KekInfo;

/**
 * PKCS#11 (HSM/TPM/SoftHSM) DEK custodian — mirrors the Rust {@code Pkcs11KeyProvider}. A
 * non-extractable AES-256 key on the token is the KEK; the DEK is wrapped with AES-256-GCM
 * <em>inside</em> the token (so the KEK never leaves hardware). The GCM AAD binds the wrapped DEK to
 * the vault id (anti-swap), exactly like {@link FileKeyProvider} — so the on-disk {@link KekInfo}
 * shape is identical (provider {@code "pkcs11"}, alg {@code "AES-256-GCM"}, wrapNonce + wrappedDek).
 *
 * <p>Uses the JDK-native {@code SunPKCS11} provider (no third-party dependency). Because SunPKCS11
 * selects a token by slot index rather than label, we discover the right slot by iterating slot
 * indices and picking the first token that logs in with the PIN <em>and</em> holds {@code keyLabel};
 * {@code tokenLabel} is kept for config parity and diagnostics.
 *
 * <p>Selected via {@code keyProvider.type = "pkcs11"} with {@code modulePath} / {@code tokenLabel} /
 * {@code keyLabel} and {@code pinEnv} (preferred) or {@code pin}.
 */
public final class Pkcs11KeyProvider implements KeyProvider {
    private static final Base64.Encoder B64E = Base64.getEncoder();
    private static final Base64.Decoder B64D = Base64.getDecoder();
    private static final int TAG_BITS = 128;
    private static final int MAX_SLOTS = 16;

    private final Provider provider;
    private final Key tokenKey;

    private Pkcs11KeyProvider(Provider provider, Key tokenKey) {
        this.provider = provider;
        this.tokenKey = tokenKey;
    }

    /** Load the module, find the slot whose token holds {@code keyLabel}, and bind that AES key. */
    public static Pkcs11KeyProvider create(String modulePath, String tokenLabel, String keyLabel, String pin) {
        Provider base = Security.getProvider("SunPKCS11");
        if (base == null) {
            throw new CredentialException("SunPKCS11 provider is unavailable in this JDK");
        }
        String lastError = null;
        for (int idx = 0; idx < MAX_SLOTS; idx++) {
            String cfg = "--name=ggvault-" + idx + "\nlibrary=" + modulePath + "\nslotListIndex=" + idx + "\n";
            try {
                Provider p = base.configure(cfg);
                KeyStore ks = KeyStore.getInstance("PKCS11", p);
                ks.load(null, pin.toCharArray());
                if (ks.containsAlias(keyLabel)) {
                    Key k = ks.getKey(keyLabel, null);
                    return new Pkcs11KeyProvider(p, k);
                }
            } catch (Exception e) {
                lastError = "slot " + idx + ": " + e.getMessage();
                // Wrong/uninitialized slot — try the next one.
            }
        }
        throw new CredentialException("pkcs11: no token with key '" + keyLabel + "' (token '" + tokenLabel
                + "', module " + modulePath + ")" + (lastError != null ? "; last error: " + lastError : ""));
    }

    @Override
    public String providerId() {
        return "pkcs11";
    }

    @Override
    public KekInfo wrapDek(String vaultId, byte[] dek) {
        try {
            byte[] iv = VaultCrypto.random(VaultCrypto.NONCE_LEN);
            Cipher c = Cipher.getInstance("AES/GCM/NoPadding", provider);
            c.init(Cipher.ENCRYPT_MODE, tokenKey, new GCMParameterSpec(TAG_BITS, iv));
            c.updateAAD(VaultFormat.dekWrapAad(vaultId));
            byte[] ct = c.doFinal(dek);
            KekInfo k = new KekInfo();
            k.provider = "pkcs11";
            k.alg = "AES-256-GCM";
            k.wrapNonce = B64E.encodeToString(iv);
            k.wrappedDek = B64E.encodeToString(ct);
            return k;
        } catch (Exception e) {
            throw new CredentialException("pkcs11 wrap: " + e.getMessage(), e);
        }
    }

    @Override
    public byte[] unwrapDek(String vaultId, KekInfo kek) {
        if (kek.wrapNonce == null) {
            throw new CredentialException("pkcs11 KEK: missing wrapNonce");
        }
        try {
            byte[] iv = B64D.decode(kek.wrapNonce);
            byte[] ct = B64D.decode(kek.wrappedDek);
            Cipher c = Cipher.getInstance("AES/GCM/NoPadding", provider);
            c.init(Cipher.DECRYPT_MODE, tokenKey, new GCMParameterSpec(TAG_BITS, iv));
            c.updateAAD(VaultFormat.dekWrapAad(vaultId));
            return c.doFinal(ct);
        } catch (Exception e) {
            throw new CredentialException("pkcs11 unwrap: " + e.getMessage(), e);
        }
    }
}
