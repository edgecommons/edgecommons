/**
 * Configuration source — SHADOW.
 *
 * Loads (and hot-reloads) configuration from an AWS IoT **named** device shadow via
 * IPC, mirroring the Java/Python/Rust `ShadowConfigProvider` contract exactly so all
 * implementations interoperate on the same shadow. A direct port of Rust `shadow.rs`.
 *
 * The component configuration is carried in the shadow as a **stringified JSON**
 * document under the `ComponentConfig` key:
 *
 * ```json
 * { "state": { "desired":  { "ComponentConfig": "<json-string of the config>" },
 *              "reported": { "ComponentConfig": "<json-string of the config>" } } }
 * ```
 *
 * - `load` reads `state.desired.ComponentConfig` (falling back to
 *   `state.reported.ComponentConfig`), parses the embedded JSON, and **reports the
 *   applied config back** into `state.reported` (verbatim string) to clear the delta.
 *   When the shadow is missing/empty it bootstraps a default config.
 * - `watch` subscribes over **local IPC pub/sub** to the shadow's
 *   `$aws/things/<thing>/shadow/name/<name>/+/+` topics (served by `ShadowManager`).
 *   On `update/delta` it applies the new config and reports it back; on `get/rejected`
 *   it reports a default config. Reacting to the *delta* (not *accepted*) is loop-safe.
 * - The shadow name defaults to the component name when `-c SHADOW` is given with no
 *   name (matching the other libraries), sanitized to AWS IoT's allowed set
 *   (`[A-Za-z0-9:_-]`) since component names contain dots, which AWS shadow names
 *   reject. An explicit name is used verbatim.
 *
 * Parity note: `extractConfigStr` returns the `ComponentConfig` string **verbatim**
 * (no re-serialization) so the reported value byte-matches `desired` and the delta
 * clears — re-serializing would reorder keys and the delta would never clear.
 */
import { EdgeCommonsError } from "../../errors";
import { Destination, Qos } from "../../messaging/types";
import { IpcMessagingProvider } from "../../messaging/ipc-provider";
import { ConfigSource, ConfigWatch } from "./index";

/**
 * The default configuration written when no shadow exists yet (mirrors the
 * Java/Python `getDefaultConfig` / `_DEFAULT_CONFIGURATION` and Rust `default_config`).
 */
/**
 * Sanitize a default shadow name to AWS IoT's allowed set (`[A-Za-z0-9:_-]`):
 * any other character (notably the `.` in a component name like
 * `com.example.Foo`) becomes `_`. Applied only to the component-name default — an
 * explicit `-c SHADOW <name>` is left verbatim. Identical across the Java/Python/
 * Rust/TS libraries so they agree on the same shadow.
 */
export function sanitizeShadowName(name: string): string {
  return name.replace(/[^A-Za-z0-9:_-]/g, "_");
}

function defaultConfig(): unknown {
  return {
    logging: {},
    tags: {},
    heartbeat: {},
    component: { global: {}, instances: [] },
  };
}

/** The default config as a compact JSON string (for reporting when no shadow exists). */
function defaultConfigStr(): string {
  try {
    return JSON.stringify(defaultConfig());
  } catch {
    return "{}";
  }
}

/**
 * Extract the component config from a full shadow document as the **verbatim JSON
 * string** stored under `ComponentConfig`: prefer `state.desired.ComponentConfig`,
 * fall back to `state.reported.ComponentConfig`. Returns `undefined` when absent.
 */
function extractConfigStr(doc: unknown): string | undefined {
  if (doc === null || typeof doc !== "object") return undefined;
  const state = (doc as Record<string, unknown>).state;
  if (state === null || typeof state !== "object") return undefined;
  for (const key of ["desired", "reported"]) {
    const section = (state as Record<string, unknown>)[key];
    if (section !== null && typeof section === "object") {
      const cfg = (section as Record<string, unknown>).ComponentConfig;
      if (typeof cfg === "string") {
        return cfg;
      }
    }
  }
  return undefined;
}

/** Greengrass-IPC-backed device-shadow configuration source (named shadow). */
export class ShadowConfigSource implements ConfigSource {
  private readonly thingName: string;
  private readonly shadowName: string;

  /**
   * @param name when `undefined`, the shadow name defaults to the component name.
   */
  constructor(
    private readonly ipc: IpcMessagingProvider,
    name: string | undefined,
    thingName: string,
    componentName: string,
  ) {
    this.thingName = thingName;
    // Explicit name verbatim; the component-name default is sanitized to a valid
    // AWS IoT shadow name (component names contain dots, which AWS rejects).
    this.shadowName = name ?? sanitizeShadowName(componentName);
  }

  /**
   * Report the applied config back into `state.reported.ComponentConfig`,
   * acknowledging the desired state and clearing the shadow delta. `componentConfig`
   * is the **stringified** config JSON, reported verbatim so it byte-matches
   * `state.desired.ComponentConfig`.
   */
  private async reportConfig(componentConfig: string): Promise<void> {
    const doc = { state: { reported: { ComponentConfig: componentConfig } } };
    try {
      const payload = Buffer.from(JSON.stringify(doc), "utf8");
      await this.ipc.updateThingShadow(this.thingName, this.shadowName, payload);
    } catch (e) {
      console.warn(`SHADOW: failed to report config back to shadow: ${(e as Error).message}`);
    }
  }

  async load(): Promise<unknown> {
    // The raw `ComponentConfig` string is reported back verbatim so it byte-matches
    // `desired` and clears the delta; it is also parsed into the config we return.
    let configStr: string;
    let bytes: Buffer | undefined;
    try {
      bytes = await this.ipc.getThingShadow(this.thingName, this.shadowName);
    } catch {
      // Shadow does not exist yet (fetch failed): bootstrap a default.
      bytes = undefined;
    }
    if (bytes !== undefined && bytes.length > 0) {
      // A non-empty shadow that is not valid JSON is a hard error (parity with Rust,
      // which propagates the parse error rather than silently defaulting).
      let doc: unknown;
      try {
        doc = JSON.parse(bytes.toString("utf8"));
      } catch (e) {
        throw EdgeCommonsError.json(`failed to parse shadow document: ${(e as Error).message}`);
      }
      configStr = extractConfigStr(doc) ?? defaultConfigStr();
    } else {
      // Shadow does not exist yet (or is empty): bootstrap a default.
      console.info(`SHADOW: no shadow document; using default config (shadow=${this.shadowName})`);
      configStr = defaultConfigStr();
    }

    // Acknowledge by reporting the applied config back verbatim (clears the delta).
    await this.reportConfig(configStr);

    try {
      return JSON.parse(configStr);
    } catch {
      return defaultConfig();
    }
  }

  sourceName(): string {
    return "SHADOW";
  }

  async watch(onUpdate: (raw: unknown) => void): Promise<ConfigWatch | undefined> {
    // Local IPC pub/sub on the shadow's event topics (served by ShadowManager).
    const filter = `$aws/things/${this.thingName}/shadow/name/${this.shadowName}/+/+`;
    const sub = await this.ipc.subscribeRaw(
      filter,
      Destination.Local,
      Qos.AtLeastOnce,
      (topic: string, payload: Buffer) => {
        // Topic suffix is `.../<action>/<result>`.
        const parts = topic.split("/");
        const result = parts[parts.length - 1] ?? "";
        const action = parts[parts.length - 2] ?? "";

        if (action === "update" && result === "delta") {
          // The delta's `state` carries the changed `ComponentConfig` (a string).
          let doc: unknown;
          try {
            doc = JSON.parse(payload.toString("utf8"));
          } catch {
            return;
          }
          if (doc === null || typeof doc !== "object") return;
          const state = (doc as Record<string, unknown>).state;
          if (state === null || typeof state !== "object") return;
          const cfgStr = (state as Record<string, unknown>).ComponentConfig;
          if (typeof cfgStr !== "string") return;
          // Report the EXACT string back to clear the delta (byte-match), then parse.
          void this.reportConfig(cfgStr).then(() => {
            try {
              onUpdate(JSON.parse(cfgStr));
            } catch {
              // Ignore an unparseable delta payload (matches Rust's silent skip).
            }
          });
        } else if (action === "get" && result === "rejected") {
          console.warn(`SHADOW: shadow missing; reporting default config (shadow=${this.shadowName})`);
          void this.reportConfig(defaultConfigStr());
        }
        // update/accepted, get/accepted, etc. — ignored.
      },
    );

    return {
      close: async () => {
        await sub.unsubscribe();
      },
    };
  }
}
