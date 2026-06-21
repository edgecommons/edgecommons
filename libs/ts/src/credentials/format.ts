/**
 * Vault on-disk format: model types + the normative byte constructions (AEAD AADs and the
 * length-prefixed canonical MAC input), matching the Rust/Python/Java references. The MAC is over
 * this byte string, not the JSON text, so JSON formatting may differ across languages.
 */
export const FORMAT_VERSION = 1;

export interface KekInfo {
  provider: string;
  alg: string;
  wrapNonce?: string;
  wrappedDek: string;
  kmsKeyId?: string;
}

export interface VersionEntry {
  version: string;
  createdMs: number;
  ttlSecs?: number;
  source: string;
  centralVersionId?: string;
  labels?: Record<string, string>;
  contentType?: string;
  nonce: string;
  ciphertext: string;
}

export interface SecretEntry {
  versions: VersionEntry[];
}

export interface VaultFile {
  format: number;
  vaultId: string;
  kek: KekInfo;
  secrets: Record<string, SecretEntry>;
  mac: string;
}

/** AEAD AAD binding a record to its vault, name, and version. */
export function recordAad(vaultId: string, name: string, version: string): Buffer {
  return Buffer.from(`ggcommons-vault/v1|${vaultId}|${name}|${version}`, "utf8");
}

/** AEAD AAD binding the wrapped DEK to its vault. */
export function dekWrapAad(vaultId: string): Buffer {
  return Buffer.from(`ggcommons-vault/v1/dek-wrap|${vaultId}`, "utf8");
}

function lp(b: Buffer): Buffer {
  const len = Buffer.alloc(4);
  len.writeUInt32LE(b.length);
  return Buffer.concat([len, b]);
}

function u32le(n: number): Buffer {
  const b = Buffer.alloc(4);
  b.writeUInt32LE(n);
  return b;
}

function u64le(n: number): Buffer {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(BigInt(n));
  return b;
}

/**
 * Build the canonical MAC input over the whole secret set. Secrets are ordered by their UTF-8 name
 * bytes (unsigned, via `Buffer.compare`) to match Rust's `BTreeMap` / Python's sort. Layout: see
 * the Rust reference `mac_input`.
 */
export function macInput(
  vaultId: string,
  secrets: Record<string, SecretEntry>,
  decodeB64: (s: string) => Buffer,
): Buffer {
  const parts: Buffer[] = [
    Buffer.from("ggcommons-vault/v1/mac", "utf8"),
    lp(Buffer.from(vaultId, "utf8")),
  ];
  const names = Object.keys(secrets).sort((a, b) =>
    Buffer.compare(Buffer.from(a, "utf8"), Buffer.from(b, "utf8")),
  );
  parts.push(u32le(names.length));
  for (const name of names) {
    parts.push(lp(Buffer.from(name, "utf8")));
    const versions = secrets[name].versions;
    parts.push(u32le(versions.length));
    for (const v of versions) {
      parts.push(lp(Buffer.from(v.version, "utf8")));
      parts.push(u64le(v.createdMs ?? 0));
      parts.push(u64le(v.ttlSecs ?? 0));
      parts.push(lp(Buffer.from(v.source ?? "", "utf8")));
      parts.push(lp(Buffer.from(v.centralVersionId ?? "", "utf8")));
      parts.push(lp(decodeB64(v.nonce)));
      parts.push(lp(decodeB64(v.ciphertext)));
    }
  }
  return Buffer.concat(parts);
}
