/**
 * Parameters (`gg.parameters()`) — an independent, offline-first service for externalized
 * configuration parameters, paralleling `credentials` (secrets).
 *
 * A pluggable {@link ParameterSource} backend (`env`, `mountedDir`, `awsSsm`) sits behind an
 * offline-first cache: remote sources persist encrypted (reusing the credentials {@link LocalVault}
 * on-disk format), local sources use an in-memory cache. Reads (`get`/`getByPath`/typed accessors)
 * always come from the cache; a background timer + on-demand `refresh()` re-pull the declared
 * names/paths. A 4th implementation alongside the Rust/Python/Java ports.
 */
export { openFromConfig, ParametersConfig, PathEntry } from "./config";
export { ParameterError } from "./errors";
export {
  DefaultParameterService,
  ParameterService,
  ParameterStats,
  SyncPath,
} from "./service";
export { EnvSource, MountedDirSource, ParamValue, ParameterSource, plainValue } from "./source";
export { AwsSsmSource } from "./ssm";
