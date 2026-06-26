package com.mbreissi.ggcommons.credentials;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.when;

import java.util.Base64;
import java.util.Map;

import org.junit.jupiter.api.Test;
import org.mockito.ArgumentCaptor;

import com.mbreissi.ggcommons.credentials.VaultModel.KekInfo;

import software.amazon.awssdk.core.SdkBytes;
import software.amazon.awssdk.services.kms.KmsClient;
import software.amazon.awssdk.services.kms.model.DecryptRequest;
import software.amazon.awssdk.services.kms.model.DecryptResponse;
import software.amazon.awssdk.services.kms.model.EncryptRequest;
import software.amazon.awssdk.services.kms.model.EncryptResponse;
import software.amazon.awssdk.services.kms.model.KmsException;

/**
 * Unit tests for {@link KmsKeyProvider} with a Mockito-mocked {@link KmsClient} — no live KMS.
 * Verifies the request shape (keyId, vaultId encryption context), the {@link KekInfo} mapping, the
 * round-trip wrap→unwrap contract, and that {@link KmsException}s are wrapped as
 * {@link CredentialException}.
 */
class KmsKeyProviderMockTest {

    private static final String KEY_ID = "arn:aws:kms:us-east-1:111122223333:key/abc";
    private static final String VAULT_ID = "vault-xyz";
    private static final byte[] DEK = "0123456789abcdef0123456789abcdef".getBytes();
    private static final byte[] CIPHERTEXT = "ENCRYPTED-DEK-BLOB".getBytes();

    @Test
    void wrapDekSendsKeyIdAndVaultIdContextAndMapsResponse() {
        KmsClient kms = mock(KmsClient.class);
        when(kms.encrypt(any(EncryptRequest.class))).thenReturn(
                EncryptResponse.builder().ciphertextBlob(SdkBytes.fromByteArray(CIPHERTEXT)).build());

        KmsKeyProvider provider = new KmsKeyProvider(kms, KEY_ID);
        KekInfo k = provider.wrapDek(VAULT_ID, DEK);

        assertEquals("kms", k.provider);
        assertEquals("aws-kms", k.alg);
        assertEquals(KEY_ID, k.kmsKeyId);
        assertEquals(Base64.getEncoder().encodeToString(CIPHERTEXT), k.wrappedDek);

        ArgumentCaptor<EncryptRequest> cap = ArgumentCaptor.forClass(EncryptRequest.class);
        org.mockito.Mockito.verify(kms).encrypt(cap.capture());
        EncryptRequest req = cap.getValue();
        assertEquals(KEY_ID, req.keyId());
        assertArrayEquals(DEK, req.plaintext().asByteArray());
        assertEquals(Map.of("vaultId", VAULT_ID), req.encryptionContext());
    }

    @Test
    void providerIdIsKms() {
        assertEquals("kms", new KmsKeyProvider(mock(KmsClient.class), KEY_ID).providerId());
    }

    @Test
    void unwrapDekSendsCiphertextKeyIdAndVaultIdContextAndReturnsPlaintext() {
        KmsClient kms = mock(KmsClient.class);
        when(kms.decrypt(any(DecryptRequest.class))).thenReturn(
                DecryptResponse.builder().plaintext(SdkBytes.fromByteArray(DEK)).build());

        KekInfo k = new KekInfo();
        k.wrappedDek = Base64.getEncoder().encodeToString(CIPHERTEXT);

        KmsKeyProvider provider = new KmsKeyProvider(kms, KEY_ID);
        assertArrayEquals(DEK, provider.unwrapDek(VAULT_ID, k));

        ArgumentCaptor<DecryptRequest> cap = ArgumentCaptor.forClass(DecryptRequest.class);
        org.mockito.Mockito.verify(kms).decrypt(cap.capture());
        DecryptRequest req = cap.getValue();
        assertEquals(KEY_ID, req.keyId());
        assertArrayEquals(CIPHERTEXT, req.ciphertextBlob().asByteArray());
        assertEquals(Map.of("vaultId", VAULT_ID), req.encryptionContext());
    }

    @Test
    void wrapThenUnwrapRoundTripThroughMock() {
        KmsClient kms = mock(KmsClient.class);
        when(kms.encrypt(any(EncryptRequest.class))).thenReturn(
                EncryptResponse.builder().ciphertextBlob(SdkBytes.fromByteArray(CIPHERTEXT)).build());
        when(kms.decrypt(any(DecryptRequest.class))).thenReturn(
                DecryptResponse.builder().plaintext(SdkBytes.fromByteArray(DEK)).build());

        KmsKeyProvider provider = new KmsKeyProvider(kms, KEY_ID);
        KekInfo k = provider.wrapDek(VAULT_ID, DEK);
        assertArrayEquals(DEK, provider.unwrapDek(VAULT_ID, k));
    }

    @Test
    void wrapWrapsKmsExceptionAsCredentialException() {
        KmsClient kms = mock(KmsClient.class);
        when(kms.encrypt(any(EncryptRequest.class)))
                .thenThrow(KmsException.builder().message("access denied").build());

        CredentialException ex = assertThrows(CredentialException.class,
                () -> new KmsKeyProvider(kms, KEY_ID).wrapDek(VAULT_ID, DEK));
        assertTrue(ex.getMessage().startsWith("kms encrypt:"));
    }

    @Test
    void unwrapWrapsKmsExceptionAsCredentialException() {
        KmsClient kms = mock(KmsClient.class);
        when(kms.decrypt(any(DecryptRequest.class)))
                .thenThrow(KmsException.builder().message("key disabled").build());

        KekInfo k = new KekInfo();
        k.wrappedDek = Base64.getEncoder().encodeToString(CIPHERTEXT);

        CredentialException ex = assertThrows(CredentialException.class,
                () -> new KmsKeyProvider(kms, KEY_ID).unwrapDek(VAULT_ID, k));
        assertTrue(ex.getMessage().startsWith("kms decrypt:"));
    }
}
