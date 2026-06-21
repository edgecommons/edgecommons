package com.aws.proserve.ggcommons.credentials;

/** AWS credentials stored as a secret (canonical camelCase JSON). */
public record AwsCredentials(String accessKeyId, String secretAccessKey, String sessionToken, String expiry) {
}
