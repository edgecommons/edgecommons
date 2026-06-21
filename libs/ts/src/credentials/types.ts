/** Secret / SecretMeta value types. */
import { CredentialError } from "./errors";

/** Non-sensitive credential-subsystem stats (for the metrics bridge). Never includes values. */
export interface CredentialStats {
  secretCount: number;
  /** Age of the last successful central sync, ms (undefined if no central sync / never synced). */
  lastSyncAgeMs?: number;
  syncFailures: number;
  rotations: number;
}

/** Metadata for a secret version — safe to log/list (no value). */
export interface SecretMeta {
  name: string;
  version: string;
  createdMs: number;
  ttlSecs?: number;
  source: string;
  labels: Record<string, string>;
}

/** A decrypted secret value plus metadata. `toJSON`/`toString` redact the value — never log it. */
export class Secret {
  constructor(
    readonly name: string,
    readonly version: string,
    private readonly value: Buffer,
    readonly labels: Record<string, string>,
    readonly createdMs: number,
    readonly source: string,
    readonly contentType: string,
  ) {}

  /** The raw secret bytes. */
  bytes(): Buffer {
    return this.value;
  }

  /** The value as UTF-8 (throws if not valid UTF-8). */
  asString(): string {
    try {
      return new TextDecoder("utf-8", { fatal: true }).decode(this.value);
    } catch {
      throw new CredentialError("secret is not valid UTF-8");
    }
  }

  /** The value parsed as JSON. */
  asJson(): unknown {
    try {
      return JSON.parse(this.asString());
    } catch (e) {
      throw new CredentialError(`secret is not JSON: ${(e as Error).message}`);
    }
  }

  toJSON(): unknown {
    return { name: this.name, version: this.version, bytes: `<${this.value.length} redacted>` };
  }

  toString(): string {
    return `Secret{name=${this.name}, version=${this.version}, bytes=<${this.value.length} redacted>}`;
  }
}
