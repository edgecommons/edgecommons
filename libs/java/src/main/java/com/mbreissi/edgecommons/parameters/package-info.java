/**
 * Parameters ({@code gg.getParameters()}) — an independent, offline-first service for externalized
 * <b>configuration parameters</b>, paralleling {@code credentials} (secrets) but for
 * non-secret-by-default settings a component reads from a central/host source.
 *
 * <p>The source is pluggable ({@link com.mbreissi.edgecommons.parameters.ParameterSource}):
 * {@code env}, {@code mountedDir} (K8s ConfigMap/Secret volumes, Docker secrets), and {@code awsSsm}
 * (SSM Parameter Store, optional dependency). The cache/refresh/typed-read machinery is identical
 * regardless of source. The cache is <b>source-aware</b>: a remote source persists encrypted
 * (reusing the credentials {@link com.mbreissi.edgecommons.credentials.LocalVault} on-disk format)
 * so values survive restarts/offline; local sources use an in-memory cache. Reads are offline-first
 * (served from the cache, never the network). {@code secure} values are never logged and cached
 * encrypted.
 *
 * <p>Java port of the normative Rust {@code parameters} module; kept at API parity across
 * Rust/Python/Java/TS.
 */
package com.mbreissi.edgecommons.parameters;
