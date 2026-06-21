/** Credential service (the public seam) + the default LocalVault-backed implementation. */
import { SyncEngine } from "./sync";
import { LocalVault, PutOptions } from "./vault";
import { Secret, SecretMeta } from "./types";

/** The public credential interface (depend on this). Obtained via `gg.credentials()`. */
export interface CredentialService {
  get(name: string): Secret | undefined;
  getVersion(name: string, version: string): Secret | undefined;
  exists(name: string): boolean;
  list(prefix?: string): SecretMeta[];
  versions(name: string): string[];
  put(name: string, value: Buffer, opts?: PutOptions): string;
  delete(name: string): boolean;
  /** Force an immediate pull from the central source (no-op without central sync). */
  refresh(): Promise<void>;

  getBytes(name: string): Buffer | undefined;
  getString(name: string): string | undefined;
  getJson(name: string): unknown | undefined;
}

/**
 * The default {@link CredentialService}: a {@link LocalVault} that refreshes any cross-process
 * change on each read. `namespace` (`<thingName>/<componentName>`) is prepended transparently to
 * every key and stripped from returned names, so a shared device vault can't collide across
 * components. Node is single-threaded, so no in-process lock is needed.
 */
export class DefaultCredentialService implements CredentialService {
  constructor(
    private readonly vault: LocalVault,
    private readonly namespace = "",
    private readonly sync?: SyncEngine,
  ) {}

  private full(name: string): string {
    return this.namespace ? `${this.namespace}/${name}` : name;
  }

  private rel(full: string): string {
    const prefix = `${this.namespace}/`;
    return this.namespace && full.startsWith(prefix) ? full.slice(prefix.length) : full;
  }

  private relName(s: Secret): Secret {
    return new Secret(this.rel(s.name), s.version, s.bytes(), s.labels, s.createdMs, s.source, s.contentType);
  }

  get(name: string): Secret | undefined {
    this.vault.reloadIfChanged();
    const s = this.vault.get(this.full(name));
    return s ? this.relName(s) : undefined;
  }

  getVersion(name: string, version: string): Secret | undefined {
    this.vault.reloadIfChanged();
    const s = this.vault.getVersion(this.full(name), version);
    return s ? this.relName(s) : undefined;
  }

  exists(name: string): boolean {
    this.vault.reloadIfChanged();
    return this.vault.exists(this.full(name));
  }

  list(prefix = ""): SecretMeta[] {
    this.vault.reloadIfChanged();
    return this.vault.list(this.full(prefix)).map((m) => ({ ...m, name: this.rel(m.name) }));
  }

  versions(name: string): string[] {
    this.vault.reloadIfChanged();
    return this.vault.versions(this.full(name));
  }

  put(name: string, value: Buffer, opts: PutOptions = {}): string {
    this.vault.reloadIfChanged();
    return this.vault.put(this.full(name), value, opts);
  }

  delete(name: string): boolean {
    this.vault.reloadIfChanged();
    return this.vault.delete(this.full(name));
  }

  async refresh(): Promise<void> {
    if (this.sync) await this.sync.syncNow();
  }

  getBytes(name: string): Buffer | undefined {
    return this.get(name)?.bytes();
  }

  getString(name: string): string | undefined {
    return this.get(name)?.asString();
  }

  getJson(name: string): unknown | undefined {
    return this.get(name)?.asJson();
  }
}
