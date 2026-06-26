/**
 * Configuration subsystem — public surface.
 *
 * Re-exports the {@link Config} model, template substitution, schema validation,
 * the {@link ConfigSource} contract, and the {@link ConfigurationChangeListener}
 * hot-reload hook (mirrors the Java/Python/Rust contract).
 */
import type { Config } from "./model";

export { Config } from "./model";
export type {
  RawConfig,
  Measures,
  HeartbeatTarget,
  ComponentConfig,
} from "./model";
export { LoggingConfig, FileLoggingConfig, HeartbeatConfig, MetricConfig, HealthConfig } from "./model";
export { resolve } from "./template";
export { validate } from "./validation";
export type { ConfigSource } from "./source";
export { buildConfigSource } from "./source";

/**
 * A listener invoked after the configuration is hot-reloaded with the new snapshot.
 * Mirrors the Java/Python `ConfigurationChangeListener` / Rust trait. Returning
 * (or resolving to) `true` indicates the change was applied.
 */
export interface ConfigurationChangeListener {
  onConfigurationChange(config: Config): Promise<boolean> | boolean;
}
