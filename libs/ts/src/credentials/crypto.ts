/**
 * Vault cryptographic primitives (must match the Rust/Python/Java references byte-for-byte).
 *
 * AES-256-GCM (96-bit nonce, 128-bit tag appended), HKDF-SHA256 for the MAC key, HMAC-SHA256 with
 * constant-time verify — all via node `crypto`. See `docs/CREDENTIALS.md` §4 and `vault-test-vectors/`.
 */
import {
  createCipheriv,
  createDecipheriv,
  createHmac,
  hkdfSync,
  randomBytes,
  timingSafeEqual,
} from "crypto";

import { CredentialError } from "./errors";

export const KEY_LEN = 32;
export const NONCE_LEN = 12;
const TAG_LEN = 16;

/** `n` cryptographically secure random bytes. */
export function random(n: number): Buffer {
  return randomBytes(n);
}

/** AES-256-GCM seal; returns `ciphertext || tag`. */
export function seal(key: Buffer, nonce: Buffer, aad: Buffer, plaintext: Buffer): Buffer {
  const c = createCipheriv("aes-256-gcm", key, nonce);
  c.setAAD(aad);
  const ct = Buffer.concat([c.update(plaintext), c.final()]);
  return Buffer.concat([ct, c.getAuthTag()]);
}

/** AES-256-GCM open of `ciphertext || tag`; throws (never returns plaintext) on failure. */
export function open(key: Buffer, nonce: Buffer, aad: Buffer, ctAndTag: Buffer): Buffer {
  if (ctAndTag.length < TAG_LEN) {
    throw new CredentialError("ciphertext too short");
  }
  const ct = ctAndTag.subarray(0, ctAndTag.length - TAG_LEN);
  const tag = ctAndTag.subarray(ctAndTag.length - TAG_LEN);
  const d = createDecipheriv("aes-256-gcm", key, nonce);
  d.setAAD(aad);
  d.setAuthTag(tag);
  try {
    return Buffer.concat([d.update(ct), d.final()]);
  } catch {
    throw new CredentialError("AEAD open failed (wrong key, tampered data, or AAD mismatch)");
  }
}

/** `HKDF-SHA256(ikm=dek, salt=vaultId, info="edgecommons-vault/v1/mac")` → 32 bytes. */
export function deriveMacKey(dek: Buffer, vaultId: string): Buffer {
  const okm = hkdfSync(
    "sha256",
    dek,
    Buffer.from(vaultId, "utf8"),
    Buffer.from("edgecommons-vault/v1/mac", "utf8"),
    KEY_LEN,
  );
  return Buffer.from(okm);
}

/** HMAC-SHA256 of `data` under `key`. */
export function hmacSha256(key: Buffer, data: Buffer): Buffer {
  return createHmac("sha256", key).update(data).digest();
}

/** Constant-time check that `HMAC-SHA256(key, data) == expected`. */
export function hmacVerify(key: Buffer, data: Buffer, expected: Buffer): boolean {
  const actual = hmacSha256(key, data);
  return actual.length === expected.length && timingSafeEqual(actual, expected);
}
