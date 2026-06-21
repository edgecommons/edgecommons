/** Credential service (the public seam) + the default LocalVault-backed implementation. */
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

  getBytes(name: string): Buffer | undefined;
  getString(name: string): string | undefined;
  getJson(name: string): unknown | undefined;
}

/**
 * The default {@link CredentialService}: a {@link LocalVault} that refreshes any cross-process
 * change on each read (the shared device vault may be written by another component). Node is
 * single-threaded, so no in-process lock is needed.
 */
export class DefaultCredentialService implements CredentialService {
  constructor(private readonly vault: LocalVault) {}

  get(name: string): Secret | undefined {
    this.vault.reloadIfChanged();
    return this.vault.get(name);
  }

  getVersion(name: string, version: string): Secret | undefined {
    this.vault.reloadIfChanged();
    return this.vault.getVersion(name, version);
  }

  exists(name: string): boolean {
    this.vault.reloadIfChanged();
    return this.vault.exists(name);
  }

  list(prefix = ""): SecretMeta[] {
    this.vault.reloadIfChanged();
    return this.vault.list(prefix);
  }

  versions(name: string): string[] {
    this.vault.reloadIfChanged();
    return this.vault.versions(name);
  }

  put(name: string, value: Buffer, opts: PutOptions = {}): string {
    this.vault.reloadIfChanged();
    return this.vault.put(name, value, opts);
  }

  delete(name: string): boolean {
    this.vault.reloadIfChanged();
    return this.vault.delete(name);
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
