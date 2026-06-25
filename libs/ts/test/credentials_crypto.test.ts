/**
 * Unit tests for the vault crypto primitives + on-disk byte format + $secret resolution.
 *
 * These assert the normative byte constructions (AES-256-GCM seal/open, HKDF-SHA256 MAC key,
 * HMAC-SHA256 constant-time verify, the length-prefixed canonical MAC input, and the AAD strings)
 * against the cross-language conformance vectors in `vault-test-vectors/`. The vectors are normative
 * across all four languages, so these checks pin TS to the Rust reference byte-for-byte.
 */
import { existsSync, readFileSync } from "fs";
import { join } from "path";

import { describe, expect, it } from "vitest";

import {
  deriveMacKey,
  hmacSha256,
  hmacVerify,
  KEY_LEN,
  NONCE_LEN,
  open,
  random,
  seal,
} from "../src/credentials/crypto";
import { CredentialError } from "../src/credentials/errors";
import { dekWrapAad, macInput, recordAad } from "../src/credentials/format";
import type { SecretEntry } from "../src/credentials/format";
import { resolveSecretRefs } from "../src/credentials/secretref";
import type { CredentialService } from "../src/credentials/service";
import { Secret } from "../src/credentials/types";

const VECTORS = join(__dirname, "..", "..", "..", "vault-test-vectors");
const unb64 = (s: string): Buffer => Buffer.from(s, "base64");

describe("crypto primitives", () => {
  it("random returns the requested number of distinct bytes", () => {
    const a = random(NONCE_LEN);
    const b = random(NONCE_LEN);
    expect(a.length).toBe(NONCE_LEN);
    expect(b.length).toBe(NONCE_LEN);
    // Vanishingly unlikely to collide; guards against a stub returning constant bytes.
    expect(a.equals(b)).toBe(false);
    expect(KEY_LEN).toBe(32);
  });

  it("seal then open is a roundtrip and yields ciphertext||tag (plaintext + 16)", () => {
    const key = random(KEY_LEN);
    const nonce = random(NONCE_LEN);
    const aad = Buffer.from("aad");
    const pt = Buffer.from("the quick brown fox");
    const ctAndTag = seal(key, nonce, aad, pt);
    expect(ctAndTag.length).toBe(pt.length + 16);
    expect(open(key, nonce, aad, ctAndTag).equals(pt)).toBe(true);
  });

  it("open with the wrong key fails closed (CredentialError, never plaintext)", () => {
    const nonce = random(NONCE_LEN);
    const aad = Buffer.from("aad");
    const ct = seal(random(KEY_LEN), nonce, aad, Buffer.from("secret"));
    expect(() => open(random(KEY_LEN), nonce, aad, ct)).toThrow(CredentialError);
    expect(() => open(random(KEY_LEN), nonce, aad, ct)).toThrow(/AEAD open failed/);
  });

  it("open with a mismatched AAD fails closed", () => {
    const key = random(KEY_LEN);
    const nonce = random(NONCE_LEN);
    const ct = seal(key, nonce, Buffer.from("aad-a"), Buffer.from("secret"));
    expect(() => open(key, nonce, Buffer.from("aad-b"), ct)).toThrow(CredentialError);
  });

  it("open of tampered ciphertext fails closed", () => {
    const key = random(KEY_LEN);
    const nonce = random(NONCE_LEN);
    const aad = Buffer.from("aad");
    const ct = seal(key, nonce, aad, Buffer.from("secret value"));
    ct[0] ^= 0x01;
    expect(() => open(key, nonce, aad, ct)).toThrow(CredentialError);
  });

  it("open rejects a buffer shorter than the GCM tag", () => {
    const key = random(KEY_LEN);
    const nonce = random(NONCE_LEN);
    expect(() => open(key, nonce, Buffer.from("aad"), Buffer.alloc(15))).toThrow(/ciphertext too short/);
    // Exactly tag-length (16) with no ciphertext is accepted by the length check but fails the AEAD.
    expect(() => open(key, nonce, Buffer.from("aad"), Buffer.alloc(16))).toThrow(/AEAD open failed/);
  });

  it("deriveMacKey is deterministic, 32 bytes, and salted by vaultId", () => {
    const dek = random(KEY_LEN);
    const k1 = deriveMacKey(dek, "vault-a");
    const k2 = deriveMacKey(dek, "vault-a");
    const k3 = deriveMacKey(dek, "vault-b");
    expect(k1.length).toBe(KEY_LEN);
    expect(k1.equals(k2)).toBe(true);
    expect(k1.equals(k3)).toBe(false);
  });

  it("hmacVerify accepts the matching tag and rejects a wrong or wrong-length tag", () => {
    const key = random(KEY_LEN);
    const data = Buffer.from("payload");
    const mac = hmacSha256(key, data);
    expect(hmacVerify(key, data, mac)).toBe(true);

    const bad = Buffer.from(mac);
    bad[0] ^= 0xff;
    expect(hmacVerify(key, data, bad)).toBe(false);

    // Length mismatch short-circuits before timingSafeEqual (which would otherwise throw).
    expect(hmacVerify(key, data, mac.subarray(0, mac.length - 1))).toBe(false);
    expect(hmacVerify(key, Buffer.from("other"), mac)).toBe(false);
  });
});

describe("on-disk byte format (AADs)", () => {
  it("recordAad binds vaultId, name, and version", () => {
    expect(recordAad("vid", "alpha", "00000001").toString("utf8")).toBe(
      "ggcommons-vault/v1|vid|alpha|00000001",
    );
  });

  it("dekWrapAad binds the wrapped DEK to its vault", () => {
    expect(dekWrapAad("vid").toString("utf8")).toBe("ggcommons-vault/v1/dek-wrap|vid");
  });

  it("macInput orders secrets by UTF-8 name bytes and applies version-field defaults", () => {
    // Insertion order beta-then-alpha must be re-sorted to alpha-then-beta in the MAC input.
    const secrets: Record<string, SecretEntry> = {
      beta: {
        versions: [
          {
            version: "00000001",
            createdMs: 5,
            source: "central",
            nonce: Buffer.from("nb").toString("base64"),
            ciphertext: Buffer.from("cb").toString("base64"),
          },
        ],
      },
      alpha: {
        // Exercises the `?? 0` / `?? ""` default branches (createdMs/ttlSecs/source/centralVersionId).
        versions: [
          {
            version: "00000001",
            createdMs: undefined as unknown as number,
            source: undefined as unknown as string,
            nonce: Buffer.from("na").toString("base64"),
            ciphertext: Buffer.from("ca").toString("base64"),
          },
        ],
      },
    };
    const mi = macInput("vid", secrets, unb64);
    const alphaIdx = mi.indexOf(Buffer.from("alpha"));
    const betaIdx = mi.indexOf(Buffer.from("beta"));
    expect(alphaIdx).toBeGreaterThan(-1);
    expect(betaIdx).toBeGreaterThan(alphaIdx); // alpha sorts before beta
    expect(mi.subarray(0, "ggcommons-vault/v1/mac".length).toString("utf8")).toBe(
      "ggcommons-vault/v1/mac",
    );
    // Stable for identical input (key order independent).
    expect(macInput("vid", { alpha: secrets.alpha, beta: secrets.beta }, unb64).equals(mi)).toBe(true);
  });

  it("macInput handles the empty secret set", () => {
    const mi = macInput("vid", {}, unb64);
    // header + length-prefixed vaultId + u32 count(0); deterministic and non-empty.
    expect(mi.length).toBeGreaterThan(0);
    expect(macInput("vid", {}, unb64).equals(mi)).toBe(true);
  });
});

describe.skipIf(!existsSync(join(VECTORS, "vectors.json")))("cross-language crypto vectors", () => {
  const vec = JSON.parse(readFileSync(join(VECTORS, "vectors.json"), "utf8"));
  const kek = unb64(vec.kekB64);
  const dek = unb64(vec.dekB64);
  const vaultId: string = vec.vaultId;

  it("reproduces the wrapped DEK from the fixed inputs", () => {
    const wrapped = seal(kek, unb64(vec.wrapNonceB64), dekWrapAad(vaultId), dek);
    expect(wrapped.toString("base64")).toBe(vec.wrappedDekB64);
    // ...and unwraps back to the canonical DEK.
    expect(open(kek, unb64(vec.wrapNonceB64), dekWrapAad(vaultId), wrapped).equals(dek)).toBe(true);
  });

  it("reproduces each record ciphertext and decrypts it back", () => {
    for (const r of vec.records) {
      const nonce = unb64(r.nonceB64);
      const pt = unb64(r.plaintextB64);
      const aad = recordAad(vaultId, r.name, r.version);
      const ct = seal(dek, nonce, aad, pt);
      expect(ct.toString("base64")).toBe(r.ciphertextB64);
      expect(open(dek, nonce, aad, unb64(r.ciphertextB64)).equals(pt)).toBe(true);
    }
  });

  it("reproduces the MAC over the canonical byte string", () => {
    const secrets: Record<string, SecretEntry> = {};
    for (const r of vec.records) {
      secrets[r.name] = {
        versions: [
          {
            version: r.version,
            createdMs: 1_700_000_000_000,
            source: "local",
            contentType: "application/octet-stream",
            nonce: r.nonceB64,
            ciphertext: r.ciphertextB64,
          },
        ],
      };
    }
    const macKey = deriveMacKey(dek, vaultId);
    const mac = hmacSha256(macKey, macInput(vaultId, secrets, unb64)).toString("base64");
    expect(mac).toBe(vec.macB64);
    // The committed MAC verifies under the derived key (constant-time path).
    expect(hmacVerify(macKey, macInput(vaultId, secrets, unb64), unb64(vec.macB64))).toBe(true);
  });
});

// ---- $secret reference resolution (covers field/non-object/null branches) ----

/** Minimal CredentialService stub returning Secrets from an in-memory map. */
function stubCreds(map: Record<string, string>): CredentialService {
  const svc = {
    get(name: string): Secret | undefined {
      if (!(name in map)) return undefined;
      return new Secret(name, "00000001", Buffer.from(map[name]), {}, 0, "local", "text/plain");
    },
  };
  return svc as unknown as CredentialService;
}

describe("resolveSecretRefs branches", () => {
  it("resolves a whole-value $secret and a JSON-field $secret", () => {
    const creds = stubCreds({ url: "postgres://h/db", aws: '{"accessKeyId":"AKIA"}' });
    const out = resolveSecretRefs(
      { a: { $secret: "url" }, b: { $secret: "aws", field: "accessKeyId" } },
      creds,
    ) as { a: string; b: string };
    expect(out.a).toBe("postgres://h/db");
    expect(out.b).toBe("AKIA");
  });

  it("passes through primitives, null, and nested arrays untouched", () => {
    const creds = stubCreds({});
    expect(resolveSecretRefs(null, creds)).toBeNull();
    expect(resolveSecretRefs(42, creds)).toBe(42);
    expect(resolveSecretRefs("x", creds)).toBe("x");
    expect(resolveSecretRefs([1, [2, "y"]], creds)).toEqual([1, [2, "y"]]);
  });

  it("ignores a non-string $secret marker (treated as an ordinary object)", () => {
    const creds = stubCreds({});
    // $secret is a number, so it is not a ref — the object is walked normally.
    const out = resolveSecretRefs({ $secret: 7, other: "keep" }, creds);
    expect(out).toEqual({ $secret: 7, other: "keep" });
  });

  it("throws when the referenced secret is absent", () => {
    const creds = stubCreds({});
    expect(() => resolveSecretRefs({ x: { $secret: "nope" } }, creds)).toThrow(CredentialError);
    expect(() => resolveSecretRefs({ x: { $secret: "nope" } }, creds)).toThrow(/not found/);
  });

  it("throws when the field is missing, not a string, or the secret is not a JSON object", () => {
    const creds = stubCreds({
      obj: '{"k":"v","num":1}',
      arr: "[1,2,3]",
      str: '"just a string"',
    });
    // field present but value is a number, not a string
    expect(() => resolveSecretRefs({ x: { $secret: "obj", field: "num" } }, creds)).toThrow(
      /field 'num' missing/,
    );
    // field absent
    expect(() => resolveSecretRefs({ x: { $secret: "obj", field: "absent" } }, creds)).toThrow(
      CredentialError,
    );
    // secret JSON is an array (not a plain object) -> field lookup fails
    expect(() => resolveSecretRefs({ x: { $secret: "arr", field: "0" } }, creds)).toThrow(
      CredentialError,
    );
    // secret JSON is a string (non-object/null branch in resolveOne)
    expect(() => resolveSecretRefs({ x: { $secret: "str", field: "k" } }, creds)).toThrow(
      CredentialError,
    );
  });
});
