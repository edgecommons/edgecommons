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
