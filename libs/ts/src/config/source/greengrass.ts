/**
 * Configuration source — GG_CONFIG.
 *
 * Loads (and hot-reloads) configuration from the Greengrass deployment via IPC
 * `GetConfiguration` + `SubscribeToConfigurationUpdate`. A port of the Rust
 * `greengrass.rs` source:
 *
 * - `load` issues `getConfiguration(keyPath, component)` and returns the subtree.
 * - The IPC key path is **empty** (whole config) when the configured `key` is empty,
 *   else the **single** configured key (it is NOT split on `.`) — matching Rust
 *   `key_path()`.
 * - `watch` registers `watchConfiguration`; because that operation delivers only the
 *   changed key path, the handler re-fetches the value and forwards the fresh doc.
 */
import { ConfigSource, ConfigWatch } from "./index";
import { IpcMessagingProvider } from "../../messaging/ipc-provider";

/** Greengrass-IPC-backed configuration source (`GetConfiguration`). */
export class GreengrassConfigSource implements ConfigSource {
  /**
   * @param ipc       the Greengrass IPC provider.
   * @param component other component to read config from, or `undefined` for this one.
   * @param key       top-level configuration key to read (e.g. `ComponentConfig`).
   */
  constructor(
    private readonly ipc: IpcMessagingProvider,
    private readonly component: string | undefined,
    private readonly key: string,
  ) {}

  /** The IPC key path: empty for the whole config, else the single configured key. */
  private keyPath(): string[] {
    return this.key === "" ? [] : [this.key];
  }

  async load(): Promise<unknown> {
    return this.ipc.getConfiguration(this.keyPath(), this.component);
  }

  sourceName(): string {
    return "GG_CONFIG";
  }

  async watch(onUpdate: (raw: unknown) => void): Promise<ConfigWatch | undefined> {
    const keyPath = this.keyPath();
    const sub = await this.ipc.watchConfiguration(keyPath, this.component, async () => {
      try {
        const value = await this.ipc.getConfiguration(keyPath, this.component);
        onUpdate(value);
      } catch (e) {
        console.warn(`GG_CONFIG watch: re-fetch failed: ${(e as Error).message}`);
      }
    });
    return {
      close: async () => {
        await sub.unsubscribe();
      },
    };
  }
}
