package com.aws.proserve.ggcommons.credentials;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.when;

import java.lang.reflect.Field;
import java.nio.charset.StandardCharsets;
import java.util.Optional;

import org.junit.jupiter.api.Test;

import software.amazon.awssdk.core.SdkBytes;
import software.amazon.awssdk.services.secretsmanager.SecretsManagerClient;
import software.amazon.awssdk.services.secretsmanager.model.GetSecretValueRequest;
import software.amazon.awssdk.services.secretsmanager.model.GetSecretValueResponse;
import software.amazon.awssdk.services.secretsmanager.model.InvalidParameterException;
import software.amazon.awssdk.services.secretsmanager.model.ResourceNotFoundException;

/**
 * Unit tests for {@link AwsSecretsManagerSource}, mocking the AWS SDK v2
 * {@link SecretsManagerClient} so no live AWS / floci is required. Covers string + binary secrets,
 * the empty/not-found path, the version-id passthrough, and the error -> {@link CredentialException}
 * mapping.
 */
class AwsSecretsManagerSourceTest {

    /**
     * Build a source with the real constructor (which builds an SDK client against a throwaway
     * endpoint), then reflectively swap in the Mockito mock so we exercise the production
     * {@link AwsSecretsManagerSource#fetch} body without touching any network.
     */
    private static AwsSecretsManagerSource sourceWith(SecretsManagerClient mock) throws Exception {
        AwsSecretsManagerSource src = new AwsSecretsManagerSource("us-east-1", "http://localhost:4566");
        Field f = AwsSecretsManagerSource.class.getDeclaredField("client");
        f.setAccessible(true);
        f.set(src, mock);
        return src;
    }

    @Test
    void fetchStringSecretReturnsBytesAndVersion() throws Exception {
        SecretsManagerClient client = mock(SecretsManagerClient.class);
        when(client.getSecretValue(any(GetSecretValueRequest.class)))
                .thenReturn(GetSecretValueResponse.builder()
                        .secretString("hello-secret")
                        .versionId("v-123")
                        .build());

        Optional<CentralSecret> got = sourceWith(client).fetch("db/password");

        assertTrue(got.isPresent());
        assertArrayEquals("hello-secret".getBytes(StandardCharsets.UTF_8), got.get().bytes());
        assertEquals("v-123", got.get().centralVersionId());
        assertTrue(got.get().labels().isEmpty());
    }

    @Test
    void fetchBinarySecretReturnsRawBytes() throws Exception {
        byte[] raw = {0, 1, 2, (byte) 0xFF, 9};
        SecretsManagerClient client = mock(SecretsManagerClient.class);
        when(client.getSecretValue(any(GetSecretValueRequest.class)))
                .thenReturn(GetSecretValueResponse.builder()
                        .secretBinary(SdkBytes.fromByteArray(raw))
                        .versionId("bin-1")
                        .build());

        Optional<CentralSecret> got = sourceWith(client).fetch("blob");

        assertTrue(got.isPresent());
        assertArrayEquals(raw, got.get().bytes());
        assertEquals("bin-1", got.get().centralVersionId());
    }

    @Test
    void missingVersionIdDefaultsToEmptyString() throws Exception {
        SecretsManagerClient client = mock(SecretsManagerClient.class);
        when(client.getSecretValue(any(GetSecretValueRequest.class)))
                .thenReturn(GetSecretValueResponse.builder().secretString("x").build());

        CentralSecret cs = sourceWith(client).fetch("k").orElseThrow();
        assertEquals("", cs.centralVersionId());
    }

    @Test
    void neitherStringNorBinaryReturnsEmpty() throws Exception {
        SecretsManagerClient client = mock(SecretsManagerClient.class);
        when(client.getSecretValue(any(GetSecretValueRequest.class)))
                .thenReturn(GetSecretValueResponse.builder().versionId("v").build());

        assertTrue(sourceWith(client).fetch("k").isEmpty());
    }

    @Test
    void resourceNotFoundMapsToEmpty() throws Exception {
        SecretsManagerClient client = mock(SecretsManagerClient.class);
        when(client.getSecretValue(any(GetSecretValueRequest.class)))
                .thenThrow(ResourceNotFoundException.builder().message("nope").build());

        assertTrue(sourceWith(client).fetch("absent").isEmpty());
    }

    @Test
    void otherSecretsManagerErrorBecomesCredentialException() throws Exception {
        SecretsManagerClient client = mock(SecretsManagerClient.class);
        when(client.getSecretValue(any(GetSecretValueRequest.class)))
                .thenThrow(InvalidParameterException.builder().message("bad param").build());

        CredentialException ex = assertThrows(CredentialException.class,
                () -> sourceWith(client).fetch("bad"));
        assertTrue(ex.getMessage().contains("bad"));
        assertTrue(ex.getMessage().contains("bad param"));
    }
}
