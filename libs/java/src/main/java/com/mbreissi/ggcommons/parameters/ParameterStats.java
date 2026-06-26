package com.mbreissi.ggcommons.parameters;

/**
 * Non-sensitive parameter-subsystem stats (safe to log/emit). Mirrors the Rust {@code ParameterStats}.
 *
 * @param parameterCount    number of cached parameters
 * @param lastRefreshAgeMs  age of the last successful refresh in ms, or {@code null} if never refreshed
 * @param refreshFailures   total refresh failures
 * @param source            the source id (e.g. {@code "env"}, {@code "mountedDir"}, {@code "awsSsm"})
 */
public record ParameterStats(
        long parameterCount,
        Long lastRefreshAgeMs,
        long refreshFailures,
        String source) {
}
