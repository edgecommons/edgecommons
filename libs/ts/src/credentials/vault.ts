/**
 * Local vault — the encrypted-at-rest secret store (TypeScript port of the Rust reference).
 *
 * Single JSON file; AES-256-GCM records; envelope-wrapped DEK; HMAC over the canonical byte string.
 * Atomic write (tmp → rename) under a cross-process directory lock for the shared device vault;
 * reload-on-change reads; fail-closed on bad KEK/tamper. Not internally synchronized — the
 * {@link DefaultCredentialService} serializes access.
 */
import {
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmdirSync,
  statSync,
  writeFileSync,
} from "fs";
import { dirname } from "path";
import { randomUUID } from "crypto";

import { deriveMacKey, hmacSha256, hmacVerify, KEY_LEN, NONCE_LEN, open, random, seal } from "./crypto";
import { CredentialError } from "./errors";
import {
  FORMAT_VERSION,
  KekInfo,
  macInput,
  recordAad,
  SecretEntry,
  VaultFile,
  VersionEntry,
} from "./format";
import { KeyProvider } from "./keyprovider";
import { Secret, SecretMeta } from "./types";

const b64 = (b: Buffer): string => b.toString("base64");
const unb64 = (s: string): Buffer => Buffer.from(s, "base64");

export interface PutOptions {
  ttlSecs?: number;
  labels?: Record<string, string>;
  contentType?: string;
  source?: string;
  centralVersionId?: string;
}

export class LocalVault {
  private constructor(
    private readonly path: string,
    private readonly vaultId: string,
    private readonly dek: Buffer,
    // retained for phase-2 KEK rotation
    private readonly keyProvider: KeyProvider,
    private kek: KekInfo,
    private secrets: Record<string, SecretEntry>,
    private readonly keep: number,
    private stamp: string | null,
  ) {
    void this.keyProvider;
  }

  /**
   * Open (or create) the vault. `newVaultId`/`newDek` let a caller supply the vault id and DEK for a
   * brand-new vault (used by the async KMS key provider, which must pre-wrap the DEK out of band);
   * they are ignored for an existing vault and default to fresh random values.
   */
  static open(
    path: string,
    keyProvider: KeyProvider,
    keepVersions = 2,
    newVaultId?: string,
    newDek?: Buffer,
  ): LocalVault {
    const keep = Math.max(1, keepVersions);
    if (existsSync(path)) {
      const vf = readFile(path);
      if (vf.format !== FORMAT_VERSION) {
        throw new CredentialError(`unsupported vault format ${vf.format}`);
      }
      const dek = keyProvider.unwrapDek(vf.vaultId, vf.kek);
      verifyMac(dek, vf);
      return new LocalVault(path, vf.vaultId, dek, keyProvider, vf.kek, vf.secrets ?? {}, keep, fileStamp(path));
    }
    const dir = dirname(path);
    if (dir) {
      mkdirSync(dir, { recursive: true });
    }
    const vaultId = newVaultId ?? randomUUID();
    const dek = newDek ?? random(KEY_LEN);
    const kek = keyProvider.wrapDek(vaultId, dek);
    const v = new LocalVault(path, vaultId, dek, keyProvider, kek, {}, keep, null);
    v.save();
    return v;
  }

  get(name: string): Secret | undefined {
    const entry = this.secrets[name];
    if (!entry || entry.versions.length === 0) {
      return undefined;
    }
    return this.decrypt(name, entry.versions[entry.versions.length - 1]);
  }

  getVersion(name: string, version: string): Secret | undefined {
    const entry = this.secrets[name];
    if (!entry) {
      return undefined;
    }
    const v = entry.versions.find((x) => x.version === version);
    return v ? this.decrypt(name, v) : undefined;
  }

  exists(name: string): boolean {
    const entry = this.secrets[name];
    return !!entry && entry.versions.length > 0;
  }

  list(prefix = ""): SecretMeta[] {
    const names = Object.keys(this.secrets).sort((a, b) =>
      Buffer.compare(Buffer.from(a, "utf8"), Buffer.from(b, "utf8")),
    );
    const out: SecretMeta[] = [];
    for (const name of names) {
      if (!name.startsWith(prefix)) {
        continue;
      }
      const vs = this.secrets[name].versions;
      if (vs.length > 0) {
        out.push(metaOf(name, vs[vs.length - 1]));
      }
    }
    return out;
  }

  versions(name: string): string[] {
    return (this.secrets[name]?.versions ?? []).map((v) => v.version);
  }

  /** Upstream version id of the latest version of `name` (for sync change detection). */
  latestCentralVersionId(name: string): string | undefined {
    return this.secrets[name]?.versions.at(-1)?.centralVersionId;
  }

  put(name: string, plaintext: Buffer, opts: PutOptions = {}): string {
    const version = this.nextVersion(name);
    const nonce = random(NONCE_LEN);
    const ct = seal(this.dek, nonce, recordAad(this.vaultId, name, version), plaintext);
    const rec: VersionEntry = {
      version,
      createdMs: Date.now(),
      source: opts.source ?? "local",
      contentType: opts.contentType ?? "application/octet-stream",
      nonce: b64(nonce),
      ciphertext: b64(ct),
    };
    if (opts.ttlSecs !== undefined) {
      rec.ttlSecs = opts.ttlSecs;
    }
    if (opts.labels && Object.keys(opts.labels).length > 0) {
      rec.labels = { ...opts.labels };
    }
    if (opts.centralVersionId !== undefined) {
      rec.centralVersionId = opts.centralVersionId;
    }
    const entry = (this.secrets[name] ??= { versions: [] });
    entry.versions.push(rec);
    if (entry.versions.length > this.keep) {
      entry.versions.splice(0, entry.versions.length - this.keep);
    }
    this.save();
    return version;
  }

  delete(name: string): boolean {
    if (this.secrets[name]) {
      delete this.secrets[name];
      this.save();
      return true;
    }
    return false;
  }

  reloadIfChanged(): boolean {
    const cur = fileStamp(this.path);
    if (cur === this.stamp) {
      return false;
    }
    const vf = readFile(this.path);
    verifyMac(this.dek, vf);
    this.secrets = vf.secrets ?? {};
    this.kek = vf.kek;
    this.stamp = cur;
    return true;
  }

  private nextVersion(name: string): string {
    const last = this.secrets[name]?.versions.at(-1)?.version;
    const n = last ? Number.parseInt(last, 10) || 0 : 0;
    return String(n + 1).padStart(8, "0");
  }

  private decrypt(name: string, v: VersionEntry): Secret {
    const pt = open(this.dek, unb64(v.nonce), recordAad(this.vaultId, name, v.version), unb64(v.ciphertext));
    return new Secret(
      name,
      v.version,
      pt,
      v.labels ?? {},
      v.createdMs,
      v.source ?? "local",
      v.contentType ?? "application/octet-stream",
    );
  }

  private save(): void {
    const macKey = deriveMacKey(this.dek, this.vaultId);
    const mac = b64(hmacSha256(macKey, macInput(this.vaultId, this.secrets, unb64)));
    const vf: VaultFile = {
      format: FORMAT_VERSION,
      vaultId: this.vaultId,
      kek: this.kek,
      secrets: this.secrets,
      mac,
    };
    const data = JSON.stringify(vf, null, 2);
    withLock(`${this.path}.lock`, () => {
      const tmp = `${this.path}.tmp`;
      writeFileSync(tmp, data);
      renameSync(tmp, this.path);
    });
    this.stamp = fileStamp(this.path);
  }
}

function readFile(path: string): VaultFile {
  let raw: string;
  try {
    raw = readFileSync(path, "utf8");
  } catch (e) {
    throw new CredentialError(`read vault: ${(e as Error).message}`);
  }
  try {
    return JSON.parse(raw) as VaultFile;
  } catch (e) {
    throw new CredentialError(`parse vault: ${(e as Error).message}`);
  }
}

function verifyMac(dek: Buffer, vf: VaultFile): void {
  const macKey = deriveMacKey(dek, vf.vaultId);
  const expected = unb64(vf.mac);
  if (!hmacVerify(macKey, macInput(vf.vaultId, vf.secrets ?? {}, unb64), expected)) {
    throw new CredentialError("vault integrity check failed (tampered or wrong key)");
  }
}

function metaOf(name: string, v: VersionEntry): SecretMeta {
  return {
    name,
    version: v.version,
    createdMs: v.createdMs,
    ttlSecs: v.ttlSecs,
    source: v.source ?? "local",
    labels: v.labels ?? {},
  };
}

function fileStamp(path: string): string | null {
  try {
    const s = statSync(path);
    return `${s.mtimeMs}:${s.size}`;
  } catch {
    return null;
  }
}

/** Cross-process advisory lock via atomic `mkdir` (no native dep), with bounded retry. */
function withLock(lockDir: string, fn: () => void): void {
  const deadline = Date.now() + 5000;
  for (;;) {
    try {
      mkdirSync(lockDir);
      break;
    } catch {
      if (Date.now() > deadline) {
        throw new CredentialError("timed out acquiring vault lock");
      }
      const until = Date.now() + 15;
      while (Date.now() < until) {
        /* brief spin-wait; writes are tiny and rare */
      }
    }
  }
  try {
    fn();
  } finally {
    try {
      rmdirSync(lockDir);
    } catch {
      /* best effort */
    }
  }
}
