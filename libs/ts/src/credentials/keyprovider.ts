/** Key providers (KEK custodians). Phase 1 ships {@link FileKeyProvider}. */
import { readFileSync, writeFileSync } from "fs";

import { KEY_LEN, NONCE_LEN, open, random, seal } from "./crypto";
import { CredentialError } from "./errors";
import { dekWrapAad, KekInfo } from "./format";

/** Wraps/unwraps the vault DEK without exposing the KEK. */
export interface KeyProvider {
  providerId(): string;
  wrapDek(vaultId: string, dek: Buffer): KekInfo;
  unwrapDek(vaultId: string, kek: KekInfo): Buffer;
}

/** KEK held as 32 bytes in a local key file (standalone / offline-fallback custodian). */
export class FileKeyProvider implements KeyProvider {
  private readonly kek: Buffer;

  constructor(kek: Buffer) {
    if (kek.length !== KEY_LEN) {
      throw new CredentialError(`KEK must be ${KEY_LEN} bytes`);
    }
    this.kek = Buffer.from(kek);
  }

  static fromKeyFile(path: string): FileKeyProvider {
    return new FileKeyProvider(readFileSync(path));
  }

  static generateKeyFile(path: string): FileKeyProvider {
    const kek = random(KEY_LEN);
    writeFileSync(path, kek, { mode: 0o600 });
    return new FileKeyProvider(kek);
  }

  providerId(): string {
    return "file";
  }

  wrapDek(vaultId: string, dek: Buffer): KekInfo {
    const nonce = random(NONCE_LEN);
    const wrapped = seal(this.kek, nonce, dekWrapAad(vaultId), dek);
    return {
      provider: "file",
      alg: "AES-256-GCM",
      wrapNonce: nonce.toString("base64"),
      wrappedDek: wrapped.toString("base64"),
    };
  }

  unwrapDek(vaultId: string, kek: KekInfo): Buffer {
    if (!kek.wrapNonce) {
      throw new CredentialError("file KEK: missing wrapNonce");
    }
    const nonce = Buffer.from(kek.wrapNonce, "base64");
    const wrapped = Buffer.from(kek.wrappedDek, "base64");
    return open(this.kek, nonce, dekWrapAad(vaultId), wrapped);
  }
}

/**
 * KMS-wrapped DEK custodian (mirrors the Rust `kms` module): the DEK is encrypted by an AWS KMS CMK
 * (the KEK never leaves KMS) and unwrapped via `kms:Decrypt` — using the default AWS credential chain
 * / TES on Greengrass. The encryption context `{vaultId}` binds the wrapped DEK to the vault id
 * (anti-swap). On-disk this produces `KekInfo{provider:"kms", alg:"aws-kms", wrappedDek:base64(ct),
 * kmsKeyId}` with no `wrapNonce`.
 *
 * ## Sync-vs-async approach
 * `KeyProvider.wrapDek`/`unwrapDek` are synchronous because `LocalVault.open` calls them inline, but
 * KMS calls are async. We resolve the round trip eagerly in {@link config!openFromConfig} (async),
 * then hand `LocalVault.open` a {@link PrewrappedKeyProvider} whose sync methods just return the
 * precomputed values. This keeps `vault.open` and the cross-language on-disk format unchanged.
 */
export class KmsKeyProvider {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private constructor(
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    private readonly client: any,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    private readonly EncryptCommand: any,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    private readonly DecryptCommand: any,
    private readonly keyId: string,
  ) {}

  /** Load `@aws-sdk/client-kms` (dynamically, so non-KMS components don't pull it) and bind a CMK. */
  static async create(keyId: string, region?: string, endpointUrl?: string): Promise<KmsKeyProvider> {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    let mod: any;
    try {
      mod = await import("@aws-sdk/client-kms");
    } catch {
      throw new CredentialError("kms key provider requires the @aws-sdk/client-kms package");
    }
    const client = new mod.KMSClient({ region, endpoint: endpointUrl });
    return new KmsKeyProvider(client, mod.EncryptCommand, mod.DecryptCommand, keyId);
  }

  providerId(): string {
    return "kms";
  }

  /** KMS-encrypt `dek` under the CMK, binding it to `vaultId` via the encryption context. */
  async wrapDek(vaultId: string, dek: Buffer): Promise<KekInfo> {
    let resp;
    try {
      resp = await this.client.send(
        new this.EncryptCommand({
          KeyId: this.keyId,
          Plaintext: dek,
          EncryptionContext: { vaultId },
        }),
      );
    } catch (e) {
      throw new CredentialError(`kms encrypt: ${(e as Error)?.message ?? String(e)}`);
    }
    if (!resp.CiphertextBlob) {
      throw new CredentialError("kms encrypt: no ciphertext");
    }
    return {
      provider: "kms",
      alg: "aws-kms",
      wrappedDek: Buffer.from(resp.CiphertextBlob).toString("base64"),
      kmsKeyId: this.keyId,
    };
  }

  /** KMS-decrypt the wrapped DEK described by `kek`, asserting the `vaultId` encryption context. */
  async unwrapDek(vaultId: string, kek: KekInfo): Promise<Buffer> {
    const ct = Buffer.from(kek.wrappedDek, "base64");
    let resp;
    try {
      resp = await this.client.send(
        new this.DecryptCommand({
          CiphertextBlob: ct,
          KeyId: this.keyId,
          EncryptionContext: { vaultId },
        }),
      );
    } catch (e) {
      throw new CredentialError(`kms decrypt: ${(e as Error)?.message ?? String(e)}`);
    }
    if (!resp.Plaintext) {
      throw new CredentialError("kms decrypt: no plaintext");
    }
    const pt = Buffer.from(resp.Plaintext);
    if (pt.length !== KEY_LEN) {
      throw new CredentialError("kms: unwrapped DEK wrong length");
    }
    return pt;
  }
}

/**
 * In-memory {@link KeyProvider} shim returning a pre-resolved KEK/DEK. Used to bridge the async KMS
 * round trip into `LocalVault.open`'s synchronous `wrapDek`/`unwrapDek` (see {@link KmsKeyProvider}).
 */
export class PrewrappedKeyProvider implements KeyProvider {
  constructor(
    private readonly id: string,
    private readonly kek: KekInfo,
    private readonly dek: Buffer,
  ) {}

  providerId(): string {
    return this.id;
  }

  wrapDek(): KekInfo {
    return this.kek;
  }

  unwrapDek(): Buffer {
    return Buffer.from(this.dek);
  }
}
