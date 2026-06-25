package com.breissinger.ggcommons.credentials;

/**
 * Non-sensitive credential-subsystem stats (for the metrics bridge). Never includes secret values.
 * Mirrors the Rust {@code CredentialStats}.
 *
 * @param secretCount    number of secrets in this component's namespace
 * @param lastSyncAgeMs  age of the last successful central sync in ms, or {@code null} if no central
 *                       sync is configured / nothing has synced yet
 * @param syncFailures   total central-fetch failures
 * @param rotations      total secrets written from central (rotations)
 */
public record CredentialStats(long secretCount, Long lastSyncAgeMs, long syncFailures, long rotations) {
}
