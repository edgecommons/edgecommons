package com.breissinger.ggcommons.credentials;

import java.net.URI;
import java.util.Base64;
import java.util.Map;

import com.breissinger.ggcommons.credentials.VaultModel.KekInfo;

import software.amazon.awssdk.core.SdkBytes;
import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.kms.KmsClient;
import software.amazon.awssdk.services.kms.model.DecryptRequest;
import software.amazon.awssdk.services.kms.model.DecryptResponse;
import software.amazon.awssdk.services.kms.model.EncryptRequest;
import software.amazon.awssdk.services.kms.model.EncryptResponse;
import software.amazon.awssdk.services.kms.model.KmsException;

/**
 * KMS-wrapped DEK custodian: the DEK is encrypted by an AWS KMS CMK (the KEK never leaves KMS) and
 * unwrapped via {@code kms:Decrypt} — using the default credential chain / TES on Greengrass. The
 * encryption context binds the wrapped DEK to the vault id (anti-swap). Mirrors the Rust
 * {@code mod kms}.
 *
 * <p>Selected via {@code keyProvider.type} = {@code "kms"} or {@code "greengrass"}; requires
 * {@code keyProvider.kmsKeyId}. The AWS SDK v2 {@code kms} artifact is optional on the classpath.
 */
public final class KmsKeyProvider implements KeyProvider {
    private static final Base64.Encoder B64E = Base64.getEncoder();
    private static final Base64.Decoder B64D = Base64.getDecoder();

    private final KmsClient client;
    private final String keyId;

    public KmsKeyProvider(String keyId, String region, String endpointUrl) {
        this.keyId = keyId;
        var b = KmsClient.builder();
        if (region != null) {
            b.region(Region.of(region));
        }
        if (endpointUrl != null) {
            b.endpointOverride(URI.create(endpointUrl));
        }
        this.client = b.build();
    }

    /** Package-private seam for unit tests: inject a (mocked) {@link KmsClient}. */
    KmsKeyProvider(KmsClient client, String keyId) {
        this.client = client;
        this.keyId = keyId;
    }

    @Override
    public String providerId() {
        return "kms";
    }

    @Override
    public KekInfo wrapDek(String vaultId, byte[] dek) {
        try {
            EncryptResponse r = client.encrypt(EncryptRequest.builder()
                    .keyId(keyId)
                    .plaintext(SdkBytes.fromByteArray(dek))
                    .encryptionContext(Map.of("vaultId", vaultId))
                    .build());
            KekInfo k = new KekInfo();
            k.provider = "kms";
            k.alg = "aws-kms";
            k.wrappedDek = B64E.encodeToString(r.ciphertextBlob().asByteArray());
            k.kmsKeyId = keyId;
            return k;
        } catch (KmsException e) {
            throw new CredentialException("kms encrypt: " + e.getMessage());
        }
    }

    @Override
    public byte[] unwrapDek(String vaultId, KekInfo kek) {
        try {
            byte[] ct = B64D.decode(kek.wrappedDek);
            DecryptResponse r = client.decrypt(DecryptRequest.builder()
                    .ciphertextBlob(SdkBytes.fromByteArray(ct))
                    .keyId(keyId)
                    .encryptionContext(Map.of("vaultId", vaultId))
                    .build());
            return r.plaintext().asByteArray();
        } catch (KmsException e) {
            throw new CredentialException("kms decrypt: " + e.getMessage());
        }
    }
}
