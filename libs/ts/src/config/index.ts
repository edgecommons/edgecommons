/**
 * Configuration subsystem — public surface.
 *
 * Re-exports the {@link Config} model, template substitution, schema validation,
 * the {@link ConfigSource} contract, and the {@link ConfigurationChangeListener}
 * hot-reload hook (mirrors the Java/Python/Rust contract).
 */
import type { Config } from "./model";
import type { JsonObject } from "./merge";

export { Config, DEFAULT_REQUEST_TIMEOUT_SECONDS } from "./model";
export type {
  RawConfig,
  Measures,
  ComponentConfig,
  LoggingPublishDestination,
  LoggingPublishLevel,
  LoggingPublishOnFull,
  LoggingPublishQueueConfig,
  LoggingPublishRedactionConfig,
} from "./model";
export {
  LoggingConfig,
  FileLoggingConfig,
  LoggingPublishConfig,
  HeartbeatConfig,
  MetricConfig,
  HealthConfig,
} from "./model";
export { resolve, sanitize } from "./template";
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

/** The lifecycle phase for a candidate configuration check. */
export enum ConfigurationValidationPhase {
  Initial = "INITIAL",
  Reload = "RELOAD",
}

/** An application validator either accepts or returns an operator-safe stable error. */
export type ConfigurationValidationResult =
  | { readonly accepted: true }
  | { readonly accepted: false; readonly error: string };

/**
 * Side-effect-free candidate validator invoked after schema validation but before the current
 * snapshot swap. Both inputs are defensive immutable JSON snapshots; the current one is redacted.
 */
export type ConfigurationValidator = (
  candidate: Readonly<JsonObject>,
  redactedCurrent: Readonly<JsonObject> | undefined,
  phase: ConfigurationValidationPhase,
) => ConfigurationValidationResult | Promise<ConfigurationValidationResult>;
