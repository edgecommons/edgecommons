/**
 * Configuration — sources.
 *
 * Pluggable {@link ConfigSource} implementations and the {@link buildConfigSource}
 * dispatch that selects one from a parsed {@link ConfigSourceSpec}. A direct port of
 * the Rust `config::source` module (`mod.rs`'s `build()` + `ConfigSource` trait),
 * mirroring its behavior exactly.
 *
 * Each source loads (and, where applicable, watches) a raw JSON config document.
 * `FILE`/`ENV` and `CONFIG_COMPONENT` work in any mode; `GG_CONFIG`/`SHADOW` require
 * the Greengrass IPC provider. Selecting a source whose dependency is missing raises
 * a {@link GgError} of kind `Config` rather than silently degrading.
 */
import { GgError } from "../../errors";
import { ConfigSourceSpec } from "../../cli";
import { IMessagingService } from "../../messaging/types";
import { IpcMessagingProvider } from "../../messaging/ipc-provider";

import { FileConfigSource } from "./file";
import { EnvConfigSource } from "./env";
import { GreengrassConfigSource } from "./greengrass";
import { ShadowConfigSource } from "./shadow";
import { ConfigComponentSource } from "./config_component";

export { FileConfigSource } from "./file";
export { EnvConfigSource } from "./env";
export { GreengrassConfigSource } from "./greengrass";
export { ShadowConfigSource } from "./shadow";
export { ConfigComponentSource } from "./config_component";

/** A live config watch; closing it stops delivering updates and releases resources. */
export interface ConfigWatch {
  close(): Promise<void>;
}

/** A source of configuration documents. */
export interface ConfigSource {
  /** Load the current raw configuration document (an object). */
  load(): Promise<unknown>;

  /** Short name of the source (for diagnostics). */
  sourceName(): string;

  /**
   * Begin watching; deliver each new raw config doc to `onUpdate`. Returns a
   * closeable handle, or `undefined` if the source can't watch (no hot reload).
   */
  watch(onUpdate: (raw: unknown) => void): Promise<ConfigWatch | undefined>;
}

/**
 * Dependencies available to a source. `messaging` is required only by
 * `CONFIG_COMPONENT`; `ipcProvider` only by `GG_CONFIG`/`SHADOW`; the other sources
 * ignore them. `thingName`/`componentName` carry the component identity.
 */
export interface BuildConfigSourceOptions {
  messaging?: IMessagingService;
  ipcProvider?: IpcMessagingProvider;
  thingName: string;
  componentName: string;
}

/**
 * Construct the configuration source for a parsed spec (mirrors Rust `build()`).
 *
 * `FILE`/`ENV` need nothing extra. `GG_CONFIG`/`SHADOW` require `opts.ipcProvider`;
 * `CONFIG_COMPONENT` requires `opts.messaging` — a missing dependency throws a
 * {@link GgError} of kind `Config`.
 */
export function buildConfigSource(spec: ConfigSourceSpec, opts: BuildConfigSourceOptions): ConfigSource {
  switch (spec.kind) {
    case "FILE":
      return new FileConfigSource(spec.path);
    case "ENV":
      return new EnvConfigSource(spec.var);
    case "CONFIG_COMPONENT": {
      if (!opts.messaging) {
        throw GgError.config(
          "CONFIG_COMPONENT source requires a messaging service (run in a mode that provides one)",
        );
      }
      return new ConfigComponentSource(opts.messaging, opts.thingName, opts.componentName);
    }
    case "GG_CONFIG": {
      if (!opts.ipcProvider) {
        throw GgError.config("GG_CONFIG source requires the Greengrass IPC provider");
      }
      return new GreengrassConfigSource(opts.ipcProvider, spec.component, spec.key);
    }
    case "SHADOW": {
      if (!opts.ipcProvider) {
        throw GgError.config("SHADOW source requires the Greengrass IPC provider");
      }
      return new ShadowConfigSource(opts.ipcProvider, spec.name, opts.thingName, opts.componentName);
    }
    default: {
      // Exhaustiveness guard: `spec` is `never` here if all kinds are handled.
      const _exhaustive: never = spec;
      throw GgError.config(`unknown config source spec: ${JSON.stringify(_exhaustive)}`);
    }
  }
}
