/**
 * Configuration source — ENV.
 *
 * Loads configuration from a JSON document held in an environment variable (default
 * `CONFIG`). A port of the Rust `env.rs` source: `load` reads + JSON-parses the
 * variable (config error if unset, JSON error if invalid). No hot reload — `watch`
 * returns `undefined` (the environment is fixed for the process lifetime).
 */
import { EdgeCommonsError } from "../../errors";
import { ConfigSource, ConfigWatch } from "./index";

/** Loads configuration from an environment variable (default `CONFIG`). */
export class EnvConfigSource implements ConfigSource {
  constructor(private readonly varName: string) {}

  async load(): Promise<unknown> {
    const raw = process.env[this.varName];
    if (raw === undefined) {
      throw EdgeCommonsError.config(`environment variable '${this.varName}' is not set`);
    }
    try {
      return JSON.parse(raw);
    } catch (e) {
      throw EdgeCommonsError.json(
        `environment variable '${this.varName}' does not contain valid JSON: ${(e as Error).message}`,
      );
    }
  }

  sourceName(): string {
    return "ENV";
  }

  async watch(_onUpdate: (raw: unknown) => void): Promise<ConfigWatch | undefined> {
    return undefined;
  }
}
