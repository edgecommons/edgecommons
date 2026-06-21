package com.aws.proserve.ggcommons.credentials;

import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.util.Map;
import java.util.Optional;

import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.secretsmanager.SecretsManagerClient;
import software.amazon.awssdk.services.secretsmanager.model.GetSecretValueRequest;
import software.amazon.awssdk.services.secretsmanager.model.GetSecretValueResponse;
import software.amazon.awssdk.services.secretsmanager.model.ResourceNotFoundException;
import software.amazon.awssdk.services.secretsmanager.model.SecretsManagerException;

/**
 * Central source backed by AWS Secrets Manager (AWS SDK v2). Auth = default credential chain (TES
 * on Greengrass); {@code endpointUrl} overrides for an emulator (floci/LocalStack) or VPC endpoint.
 */
public final class AwsSecretsManagerSource implements CentralVaultSource {
    private final SecretsManagerClient client;

    public AwsSecretsManagerSource(String region, String endpointUrl) {
        var b = SecretsManagerClient.builder();
        if (region != null) {
            b.region(Region.of(region));
        }
        if (endpointUrl != null) {
            b.endpointOverride(URI.create(endpointUrl));
        }
        this.client = b.build();
    }

    @Override
    public Optional<CentralSecret> fetch(String name) {
        try {
            GetSecretValueResponse r = client.getSecretValue(GetSecretValueRequest.builder().secretId(name).build());
            byte[] data;
            if (r.secretString() != null) {
                data = r.secretString().getBytes(StandardCharsets.UTF_8);
            } else if (r.secretBinary() != null) {
                data = r.secretBinary().asByteArray();
            } else {
                return Optional.empty();
            }
            String version = r.versionId() != null ? r.versionId() : "";
            return Optional.of(new CentralSecret(data, version, Map.of()));
        } catch (ResourceNotFoundException e) {
            return Optional.empty();
        } catch (SecretsManagerException e) {
            throw new CredentialException("get secret '" + name + "': " + e.getMessage());
        }
    }
}
