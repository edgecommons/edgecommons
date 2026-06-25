/**
 * Unit tests for the credentials KeyProviders (KEK custodians) in src/credentials/keyprovider.ts.
 *
 * - FileKeyProvider: real AES-256-GCM wrap/unwrap round trip + on-disk key file, no mocks.
 * - KmsKeyProvider: @aws-sdk/client-kms mocked so the async wrap/unwrap logic runs without AWS.
 * - PrewrappedKeyProvider: the sync shim that returns pre-resolved KEK/DEK.
 * - Pkcs11KeyProvider: graphene-pk11 mocked so slot/login/find + wrap/unwrap logic runs without an HSM.
 */
import { mkdtempSync, readFileSync, statSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { KEY_LEN } from "../src/credentials/crypto";
import { CredentialError } from "../src/credentials/errors";
import { KekInfo } from "../src/credentials/format";

// --- KMS SDK mock ----------------------------------------------------------------------------
// Capture commands and let the test drive responses. The mock encrypts by base64-prefixing the
// plaintext and "decrypts" by stripping the prefix, while echoing the EncryptionContext so the
// provider's vaultId binding can be asserted.
const kmsCalls: Array<{ kind: string; input: Record<string, unknown> }> = [];
let kmsEncryptResp: ((input: Record<string, unknown>) => unknown) | null = null;
let kmsDecryptResp: ((input: Record<string, unknown>) => unknown) | null = null;

vi.mock("@aws-sdk/client-kms", () => {
  class EncryptCommand {
    constructor(public input: Record<string, unknown>) {}
  }
  class DecryptCommand {
    constructor(public input: Record<string, unknown>) {}
  }
  class KMSClient {
    constructor(public cfg: unknown) {}
    async send(cmd: { input: Record<string, unknown> }): Promise<unknown> {
      if (cmd instanceof EncryptCommand) {
        kmsCalls.push({ kind: "encrypt", input: cmd.input });
        return kmsEncryptResp ? kmsEncryptResp(cmd.input) : {};
      }
      kmsCalls.push({ kind: "decrypt", input: cmd.input });
      return kmsDecryptResp ? kmsDecryptResp(cmd.input) : {};
    }
  }
  return { KMSClient, EncryptCommand, DecryptCommand };
});

// --- graphene-pk11 mock ----------------------------------------------------------------------
// A tiny fake of the synchronous graphene API surface the provider touches.
const grapheneState: {
  initializeThrows: Error | null;
  loginThrows: Error | null;
  tokens: string[]; // token labels, one slot each
  keyLabels: string[]; // key labels present on the matched token
  wrapThrows: Error | null;
  unwrapThrows: Error | null;
} = {
  initializeThrows: null,
  loginThrows: null,
  tokens: ["my-token"],
  keyLabels: ["my-key"],
  wrapThrows: null,
  unwrapThrows: null,
};

vi.mock("graphene-pk11", () => {
  class AesGcmParams {
    constructor(
      public iv: Buffer,
      public aad: Buffer,
      public tagBits: number,
    ) {}
  }
  const SessionFlag = { SERIAL_SESSION: 4, RW_SESSION: 2 };
  const ObjectClass = { SECRET_KEY: 4 };

  // The "token key" XORs each byte with 0xAA to emulate a reversible wrap; the GCM tag (16 bytes)
  // is faked by appending 16 zero bytes on wrap and dropping them on unwrap.
  function xor(buf: Buffer): Buffer {
    const out = Buffer.alloc(buf.length);
    for (let i = 0; i < buf.length; i++) out[i] = buf[i] ^ 0xaa;
    return out;
  }

  const session = {
    login(_pin: string): void {
      if (grapheneState.loginThrows) throw grapheneState.loginThrows;
    },
    find(_filter: unknown): { length: number; items(i: number): unknown } {
      const labels = grapheneState.keyLabels;
      return {
        length: labels.length,
        items(_i: number) {
          return { toType: () => ({ label: labels[_i] }) };
        },
      };
    },
    createCipher(_alg: unknown, _key: unknown) {
      return {
        once(plaintext: Buffer, _out: Buffer): Buffer {
          if (grapheneState.wrapThrows) throw grapheneState.wrapThrows;
          return Buffer.concat([xor(plaintext), Buffer.alloc(16)]);
        },
      };
    },
    createDecipher(_alg: unknown, _key: unknown) {
      return {
        once(ct: Buffer, _out: Buffer): Buffer {
          if (grapheneState.unwrapThrows) throw grapheneState.unwrapThrows;
          return xor(ct.subarray(0, ct.length - 16));
        },
      };
    },
  };

  function makeSlot(tokenLabel: string) {
    return {
      getToken() {
        return { label: tokenLabel };
      },
      open(_flags: number) {
        return session;
      },
    };
  }

  const Module = {
    load(_path: string, _name: string) {
      return {
        initialize(): void {
          if (grapheneState.initializeThrows) throw grapheneState.initializeThrows;
        },
        getSlots(_tokenPresent: boolean) {
          const labels = grapheneState.tokens;
          return {
            length: labels.length,
            items(i: number) {
              return makeSlot(labels[i]);
            },
          };
        },
      };
    },
  };

  return { default: { Module, SessionFlag, ObjectClass, AesGcmParams }, Module, SessionFlag, ObjectClass, AesGcmParams };
});

// Import after mocks are registered.
import {
  FileKeyProvider,
  KmsKeyProvider,
  Pkcs11KeyProvider,
  PrewrappedKeyProvider,
} from "../src/credentials/keyprovider";

beforeEach(() => {
  kmsCalls.length = 0;
  kmsEncryptResp = null;
  kmsDecryptResp = null;
  grapheneState.initializeThrows = null;
  grapheneState.loginThrows = null;
  grapheneState.tokens = ["my-token"];
  grapheneState.keyLabels = ["my-key"];
  grapheneState.wrapThrows = null;
  grapheneState.unwrapThrows = null;
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("FileKeyProvider", () => {
  const kek = Buffer.alloc(KEY_LEN, 7);

  it("reports providerId 'file'", () => {
    expect(new FileKeyProvider(kek).providerId()).toBe("file");
  });

  it("rejects a KEK of the wrong length", () => {
    expect(() => new FileKeyProvider(Buffer.alloc(16))).toThrow(CredentialError);
    expect(() => new FileKeyProvider(Buffer.alloc(16))).toThrow(/must be 32 bytes/);
  });

  it("copies the KEK rather than aliasing the caller's buffer", () => {
    const mutable = Buffer.alloc(KEY_LEN, 1);
    const p = new FileKeyProvider(mutable);
    const dek = Buffer.alloc(KEY_LEN, 9);
    const wrapped = p.wrapDek("v1", dek);
    mutable.fill(0xff); // mutate the original buffer after construction
    // Unwrap still works -> provider kept its own copy.
    expect(p.unwrapDek("v1", wrapped)).toEqual(dek);
  });

  it("wraps then unwraps a DEK (round trip) with the kms-less KekInfo shape", () => {
    const p = new FileKeyProvider(kek);
    const dek = Buffer.alloc(KEY_LEN, 42);
    const info = p.wrapDek("vault-A", dek);
    expect(info.provider).toBe("file");
    expect(info.alg).toBe("AES-256-GCM");
    expect(info.wrapNonce).toBeTruthy();
    expect(info.wrappedDek).toBeTruthy();
    expect(info.kmsKeyId).toBeUndefined();
    expect(p.unwrapDek("vault-A", info)).toEqual(dek);
  });

  it("uses a fresh nonce each wrap (non-deterministic ciphertext)", () => {
    const p = new FileKeyProvider(kek);
    const dek = Buffer.alloc(KEY_LEN, 3);
    const a = p.wrapDek("v", dek);
    const b = p.wrapDek("v", dek);
    expect(a.wrapNonce).not.toBe(b.wrapNonce);
    expect(a.wrappedDek).not.toBe(b.wrappedDek);
  });

  it("fails to unwrap under a different vaultId (AAD binding)", () => {
    const p = new FileKeyProvider(kek);
    const dek = Buffer.alloc(KEY_LEN, 11);
    const info = p.wrapDek("vault-A", dek);
    expect(() => p.unwrapDek("vault-B", info)).toThrow(CredentialError);
  });

  it("throws when the KekInfo is missing wrapNonce", () => {
    const p = new FileKeyProvider(kek);
    const info: KekInfo = { provider: "file", alg: "AES-256-GCM", wrappedDek: "AAAA" };
    expect(() => p.unwrapDek("v", info)).toThrow(/missing wrapNonce/);
  });

  it("generateKeyFile writes a 32-byte key (mode 600) usable for round trips", () => {
    const dir = mkdtempSync(join(tmpdir(), "ggkp-gen-"));
    const path = join(dir, "vault.key");
    const p = FileKeyProvider.generateKeyFile(path);
    const onDisk = readFileSync(path);
    expect(onDisk.length).toBe(KEY_LEN);
    const dek = Buffer.alloc(KEY_LEN, 8);
    expect(p.unwrapDek("v", p.wrapDek("v", dek))).toEqual(dek);
    // Mode bits (low 9) — skip the assertion where the platform ignores them (Windows).
    if (process.platform !== "win32") {
      expect(statSync(path).mode & 0o777).toBe(0o600);
    }
  });

  it("fromKeyFile loads a key persisted by generateKeyFile (interop between instances)", () => {
    const dir = mkdtempSync(join(tmpdir(), "ggkp-load-"));
    const path = join(dir, "vault.key");
    const writer = FileKeyProvider.generateKeyFile(path);
    const reader = FileKeyProvider.fromKeyFile(path);
    const dek = Buffer.alloc(KEY_LEN, 12);
    // wrapped by writer -> unwrapped by an independently-loaded reader.
    expect(reader.unwrapDek("v", writer.wrapDek("v", dek))).toEqual(dek);
  });
});

describe("PrewrappedKeyProvider", () => {
  it("returns the pre-resolved id, KEK and a copy of the DEK", () => {
    const kek: KekInfo = { provider: "kms", alg: "aws-kms", wrappedDek: "Y3Q=", kmsKeyId: "k1" };
    const dek = Buffer.alloc(KEY_LEN, 1);
    const p = new PrewrappedKeyProvider("kms", kek, dek);
    expect(p.providerId()).toBe("kms");
    expect(p.wrapDek()).toBe(kek);
    const got = p.unwrapDek();
    expect(got).toEqual(dek);
    // Defensive copy: mutating the returned buffer must not corrupt the stored DEK.
    got.fill(0xff);
    expect(p.unwrapDek()).toEqual(dek);
  });
});

describe("KmsKeyProvider (mocked @aws-sdk/client-kms)", () => {
  async function provider(): Promise<KmsKeyProvider> {
    return KmsKeyProvider.create("arn:aws:kms:us-east-1:0:key/k1", "us-east-1", "http://localhost:4566");
  }

  it("reports providerId 'kms'", async () => {
    expect((await provider()).providerId()).toBe("kms");
  });

  it("wrapDek KMS-encrypts the DEK, binds vaultId, and returns the kms KekInfo shape", async () => {
    const p = await provider();
    const dek = Buffer.alloc(KEY_LEN, 5);
    kmsEncryptResp = () => ({ CiphertextBlob: Buffer.from("CIPHERTEXT") });
    const info = await p.wrapDek("vault-X", dek);
    expect(kmsCalls).toHaveLength(1);
    expect(kmsCalls[0].kind).toBe("encrypt");
    expect(kmsCalls[0].input.KeyId).toBe("arn:aws:kms:us-east-1:0:key/k1");
    expect(kmsCalls[0].input.EncryptionContext).toEqual({ vaultId: "vault-X" });
    expect(Buffer.from(kmsCalls[0].input.Plaintext as Buffer)).toEqual(dek);
    expect(info.provider).toBe("kms");
    expect(info.alg).toBe("aws-kms");
    expect(info.kmsKeyId).toBe("arn:aws:kms:us-east-1:0:key/k1");
    expect(info.wrapNonce).toBeUndefined();
    expect(Buffer.from(info.wrappedDek, "base64").toString()).toBe("CIPHERTEXT");
  });

  it("wrapDek throws CredentialError when KMS returns no ciphertext", async () => {
    const p = await provider();
    kmsEncryptResp = () => ({}); // no CiphertextBlob
    await expect(p.wrapDek("v", Buffer.alloc(KEY_LEN))).rejects.toThrow(/no ciphertext/);
  });

  it("wrapDek wraps a KMS send error in a CredentialError", async () => {
    const p = await provider();
    kmsEncryptResp = () => {
      throw new Error("AccessDenied");
    };
    await expect(p.wrapDek("v", Buffer.alloc(KEY_LEN))).rejects.toThrow(/kms encrypt: AccessDenied/);
  });

  it("unwrapDek KMS-decrypts the wrapped DEK and asserts the vaultId context", async () => {
    const p = await provider();
    const dek = Buffer.alloc(KEY_LEN, 9);
    kmsDecryptResp = () => ({ Plaintext: dek });
    const kek: KekInfo = {
      provider: "kms",
      alg: "aws-kms",
      wrappedDek: Buffer.from("CT").toString("base64"),
      kmsKeyId: "k1",
    };
    const got = await p.unwrapDek("vault-Y", kek);
    expect(got).toEqual(dek);
    expect(kmsCalls[0].kind).toBe("decrypt");
    expect(kmsCalls[0].input.EncryptionContext).toEqual({ vaultId: "vault-Y" });
    expect(Buffer.from(kmsCalls[0].input.CiphertextBlob as Buffer).toString()).toBe("CT");
  });

  it("unwrapDek throws when KMS returns no plaintext", async () => {
    const p = await provider();
    kmsDecryptResp = () => ({});
    await expect(p.unwrapDek("v", { provider: "kms", alg: "aws-kms", wrappedDek: "AA==" })).rejects.toThrow(
      /no plaintext/,
    );
  });

  it("unwrapDek throws when the unwrapped DEK is the wrong length", async () => {
    const p = await provider();
    kmsDecryptResp = () => ({ Plaintext: Buffer.alloc(16) }); // != KEY_LEN
    await expect(p.unwrapDek("v", { provider: "kms", alg: "aws-kms", wrappedDek: "AA==" })).rejects.toThrow(
      /wrong length/,
    );
  });

  it("unwrapDek wraps a KMS send error in a CredentialError", async () => {
    const p = await provider();
    kmsDecryptResp = () => {
      throw new Error("KeyDisabled");
    };
    await expect(p.unwrapDek("v", { provider: "kms", alg: "aws-kms", wrappedDek: "AA==" })).rejects.toThrow(
      /kms decrypt: KeyDisabled/,
    );
  });
});

describe("Pkcs11KeyProvider (mocked graphene-pk11)", () => {
  async function provider(): Promise<Pkcs11KeyProvider> {
    return Pkcs11KeyProvider.create("/usr/lib/softhsm.so", "my-token", "my-key", "1234");
  }

  it("reports providerId 'pkcs11' and round-trips a DEK wrapped on the token", async () => {
    const p = await provider();
    expect(p.providerId()).toBe("pkcs11");
    const dek = Buffer.alloc(KEY_LEN, 22);
    const info = p.wrapDek("vault-Z", dek);
    expect(info.provider).toBe("pkcs11");
    expect(info.alg).toBe("AES-256-GCM");
    expect(info.wrapNonce).toBeTruthy();
    expect(info.wrappedDek).toBeTruthy();
    expect(info.kmsKeyId).toBeUndefined();
    expect(p.unwrapDek("vault-Z", info)).toEqual(dek);
  });

  it("tolerates CKR_CRYPTOKI_ALREADY_INITIALIZED on module.initialize", async () => {
    grapheneState.initializeThrows = new Error("CKR_CRYPTOKI_ALREADY_INITIALIZED");
    await expect(provider()).resolves.toBeInstanceOf(Pkcs11KeyProvider);
  });

  it("surfaces a non-already-initialized initialize failure", async () => {
    grapheneState.initializeThrows = new Error("CKR_GENERAL_ERROR");
    await expect(provider()).rejects.toThrow(/pkcs11 initialize: CKR_GENERAL_ERROR/);
  });

  it("tolerates CKR_USER_ALREADY_LOGGED_IN on session.login", async () => {
    grapheneState.loginThrows = new Error("CKR_USER_ALREADY_LOGGED_IN");
    await expect(provider()).resolves.toBeInstanceOf(Pkcs11KeyProvider);
  });

  it("surfaces a non-already-logged-in login failure", async () => {
    grapheneState.loginThrows = new Error("CKR_PIN_INCORRECT");
    await expect(provider()).rejects.toThrow(/pkcs11 login: CKR_PIN_INCORRECT/);
  });

  it("errors when no token carries the requested label", async () => {
    grapheneState.tokens = ["other-token"];
    await expect(provider()).rejects.toThrow(/no token labelled 'my-token'/);
  });

  it("errors when the key label is not found on the token", async () => {
    grapheneState.keyLabels = [];
    await expect(provider()).rejects.toThrow(/no key labelled 'my-key'/);
  });

  it("wraps a token wrap failure in a CredentialError", async () => {
    const p = await provider();
    grapheneState.wrapThrows = new Error("CKR_DEVICE_ERROR");
    expect(() => p.wrapDek("v", Buffer.alloc(KEY_LEN))).toThrow(/pkcs11 wrap: CKR_DEVICE_ERROR/);
  });

  it("unwrapDek throws when the KekInfo is missing wrapNonce", async () => {
    const p = await provider();
    expect(() => p.unwrapDek("v", { provider: "pkcs11", alg: "AES-256-GCM", wrappedDek: "AA==" })).toThrow(
      /missing wrapNonce/,
    );
  });

  it("wraps a token unwrap failure in a CredentialError", async () => {
    const p = await provider();
    const info = p.wrapDek("v", Buffer.alloc(KEY_LEN));
    grapheneState.unwrapThrows = new Error("CKR_DEVICE_ERROR");
    expect(() => p.unwrapDek("v", info)).toThrow(/pkcs11 unwrap: CKR_DEVICE_ERROR/);
  });
});

describe("optional-dependency missing (dynamic import fails)", () => {
  // Re-import the module under a reset registry with the optional SDKs mocked to throw on import,
  // exercising the "requires the … package" catch branches in create().
  afterEach(() => {
    vi.resetModules();
    vi.doUnmock("@aws-sdk/client-kms");
    vi.doUnmock("graphene-pk11");
  });

  it("KmsKeyProvider.create errors clearly when @aws-sdk/client-kms is absent", async () => {
    vi.resetModules();
    vi.doMock("@aws-sdk/client-kms", () => {
      throw new Error("Cannot find module");
    });
    const kp = await import("../src/credentials/keyprovider");
    await expect(kp.KmsKeyProvider.create("k1")).rejects.toThrow(
      /requires the @aws-sdk\/client-kms package/,
    );
  });

  it("Pkcs11KeyProvider.create errors clearly when graphene-pk11 is absent", async () => {
    vi.resetModules();
    vi.doMock("graphene-pk11", () => {
      throw new Error("Cannot find module");
    });
    const kp = await import("../src/credentials/keyprovider");
    await expect(kp.Pkcs11KeyProvider.create("/m.so", "t", "k", "pin")).rejects.toThrow(
      /requires the graphene-pk11 package/,
    );
  });
});
