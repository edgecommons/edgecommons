package com.aws.proserve.ggcommons.parameters;

import java.util.List;
import java.util.Map;
import java.util.Optional;

/**
 * The pluggable parameter backend — the seam the parameter service reads from (AWS SSM, a mounted
 * directory, env vars, or a custom host-supplied source). The service (cache, refresh, typed reads)
 * is identical regardless of source. Implementations must be thread-safe. Mirrors the Rust
 * {@code ParameterSource} trait.
 */
public interface ParameterSource {

    /** Fetch one parameter by name, or empty if it does not exist. */
    Optional<ParamValue> fetch(String name);

    /** Fetch every parameter under {@code path} (recursively when {@code recursive}). Empty when absent. */
    List<Map.Entry<String, ParamValue>> fetchByPath(String path, boolean recursive);

    /** Stable id for diagnostics/stats (e.g. {@code "awsSsm"}, {@code "mountedDir"}, {@code "env"}). */
    String sourceId();
}
