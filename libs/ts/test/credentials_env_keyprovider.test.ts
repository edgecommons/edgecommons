/**
 * Phase 1d env KeyProvider (FR-CRED-3 / FR-CRED-6) conformance tests.
 *
 * Self-contained (no shared `vault-test-vectors/` files, no AWS mocks): the env provider sources the
 * vault KEK as a RAW 32-byte key, base64-encoded, from an environment variable. Coverage mirrors the
 * locked design:
 *   (a) round-trip through the config path (`type: "env"`): put → reopen → get;
 *   (b) crypto-identity with {@link FileKeyProvider} given the SAME raw KEK (provider-level + full-vault);
 *   (c) error cases — env var unset / empty / invalid base64 / wrong decoded length;
 *   (d) buildKeyProvider default-type precedence (explicit ▸ profile default ▸ library default `file`).
 */
import { mkdtempSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { buildKeyProvider, openFromConfig } from "../src/credentials/config";
import { KEY_LEN } from "../src/credentials/crypto";
import { CredentialError } from "../src/credentials/errors";
import { EnvKeyProvider, FileKeyProvider } from "../src/credentials/keyprovider";
import { LocalVault } from "../src/credentials/vault";

function dir(prefix: string): string {
  return mkdtempSync(join(tmpdir(), prefix));
}

/** A known, fixed 32-byte KEK and its base64 encoding (the value a mounted k8s Secret would carry). */
const KEK = Buffer.alloc(KEY_LEN, 7);
const KEK_B64 = KEK.toString("base64");

/** A test-private env var name so we never collide with a real `GGCOMMONS_VAULT_KEK` in the env. */
const VAR = "GGCOMMONS_TEST_VAULT_KEK";

beforeEach(() => {
  delete process.env[VAR];
  delete process.env.GGCOMMONS_VAULT_KEK;
});
afterEach(() => {
  delete process.env[VAR];
  delete process.env.GGCOMMONS_VAULT_KEK;
});

describe("EnvKeyProvider.fromEnv", () => {
  it("reports providerId 'env' and writes a KekInfo tagged provider:'env'", () => {
    process.env[VAR] = KEK_B64;
    const p = EnvKeyProvider.fromEnv(VAR);
    expect(p.providerId()).toBe("env");
    const info = p.wrapDek("vault-A", Buffer.alloc(KEY_LEN, 9));
    expect(info.provider).toBe("env");
    expect(info.alg).toBe("AES-256-GCM");
    expect(info.wrapNonce).toBeTruthy();
    expect(info.kmsKeyId).toBeUndefined();
  });

  it("round-trips a DEK (wrap then unwrap) under the env-sourced KEK", () => {
    process.env[VAR] = KEK_B64;
    const p = EnvKeyProvider.fromEnv(VAR);
    const dek = Buffer.alloc(KEY_LEN, 42);
    expect(p.unwrapDek("vault-A", p.wrapDek("vault-A", dek))).toEqual(dek);
  });

  it("tolerates a trailing newline (KEK sourced from a mounted file/Secret)", () => {
    // Cross-language parity: Java (b64.trim()) and Rust (raw.trim()) accept a trailing newline, so a
    // Secret value like `echo -n <key> | base64` (often carrying a \n) must decode identically here.
    process.env[VAR] = KEK_B64 + "\n";
    const p = EnvKeyProvider.fromEnv(VAR);
    expect(p.providerId()).toBe("env");
    const dek = Buffer.alloc(KEY_LEN, 42);
    expect(p.unwrapDek("vault-A", p.wrapDek("vault-A", dek))).toEqual(dek);
  });

  it("errors clearly when the env var is unset", () => {
    expect(() => EnvKeyProvider.fromEnv(VAR)).toThrow(CredentialError);
    expect(() => EnvKeyProvider.fromEnv(VAR)).toThrow(/unset or empty/);
  });

  it("errors clearly when the env var is empty", () => {
    process.env[VAR] = "";
    expect(() => EnvKeyProvider.fromEnv(VAR)).toThrow(/unset or empty/);
  });

  it("errors clearly when the env var is not valid base64", () => {
    process.env[VAR] = "not valid base64!!"; // illegal chars + whitespace
    expect(() => EnvKeyProvider.fromEnv(VAR)).toThrow(/not valid base64/);
  });

  it("errors clearly when the decoded key is the wrong length", () => {
    process.env[VAR] = Buffer.alloc(16, 1).toString("base64"); // 16 bytes, valid base64
    expect(() => EnvKeyProvider.fromEnv(VAR)).toThrow(/must be 32 bytes/);
  });

  it("the constructor rejects a raw KEK of the wrong length (delegates to FileKeyProvider)", () => {
    expect(() => new EnvKeyProvider(Buffer.alloc(16))).toThrow(/must be 32 bytes/);
  });
});

describe("EnvKeyProvider crypto-identity with FileKeyProvider (same raw KEK)", () => {
  it("a DEK wrapped by Env(K) unwraps under File(K), and vice-versa", () => {
    process.env[VAR] = KEK_B64;
    const env = EnvKeyProvider.fromEnv(VAR);
    const file = new FileKeyProvider(KEK);
    const dek = Buffer.alloc(KEY_LEN, 123);

    // Env-wrapped → File-unwrapped (the on-disk wrapped bytes are byte-compatible).
    const wrappedByEnv = env.wrapDek("vault-A", dek);
    expect(wrappedByEnv.provider).toBe("env");
    expect(file.unwrapDek("vault-A", wrappedByEnv)).toEqual(dek);

    // File-wrapped → Env-unwrapped (the crypto path is identical; only the provider tag differs).
    const wrappedByFile = file.wrapDek("vault-A", dek);
    expect(wrappedByFile.provider).toBe("file");
    expect(wrappedByFile.alg).toBe(wrappedByEnv.alg);
    expect(env.unwrapDek("vault-A", wrappedByFile)).toEqual(dek);
  });

  it("a vault created under EnvKeyProvider(K) opens under FileKeyProvider(K)", async () => {
    const d = dir("ggenv-xid-");
    const path = join(d, "vault");
    process.env[VAR] = KEK_B64;

    // Create + populate the vault via the config (env) path.
    const svc = await openFromConfig({ vault: { path, keyProvider: { type: "env", envVar: VAR } } });
    svc.put("db/password", Buffer.from("s3cr3t"));
    expect(svc.getString("db/password")).toBe("s3cr3t");

    // Reopen the SAME on-disk vault with a *FileKeyProvider* built from the same raw KEK — proves the
    // wrapped DEK + MAC are byte-compatible (the env provider's `provider:"env"` tag is ignored on read).
    const reopened = LocalVault.open(path, new FileKeyProvider(KEK));
    expect(reopened.get("db/password")?.asString()).toBe("s3cr3t");
  });
});

describe("openFromConfig via the env key provider", () => {
  it("round-trips a secret across a close+reopen (type: 'env')", async () => {
    const d = dir("ggenv-rt-");
    const path = join(d, "vault");
    process.env[VAR] = KEK_B64;

    const c1 = await openFromConfig({ vault: { path, keyProvider: { type: "env", envVar: VAR } } });
    c1.put("api/token", Buffer.from("tok-1"));

    // Reopen the persisted vault under a fresh service + env provider (env var still set).
    const c2 = await openFromConfig({ vault: { path, keyProvider: { type: "env", envVar: VAR } } });
    expect(c2.getString("api/token")).toBe("tok-1");
  });

  it("defaults the env var name to GGCOMMONS_VAULT_KEK when `envVar` is omitted", async () => {
    const d = dir("ggenv-defvar-");
    const path = join(d, "vault");
    process.env.GGCOMMONS_VAULT_KEK = KEK_B64;

    const svc = await openFromConfig({ vault: { path, keyProvider: { type: "env" } } });
    svc.put("k", Buffer.from("v"));
    expect(svc.getString("k")).toBe("v");
  });

  it("propagates an env-provider error (missing var) out of openFromConfig", async () => {
    const d = dir("ggenv-err-");
    await expect(
      openFromConfig({ vault: { path: join(d, "vault"), keyProvider: { type: "env", envVar: VAR } } }),
    ).rejects.toThrow(/GGCOMMONS_TEST_VAULT_KEK.*unset or empty/);
  });
});

describe("buildKeyProvider default-type precedence (Phase 1d / FR-CRED-6, FR-RT-3)", () => {
  it("uses the env provider when type is ABSENT and the profile default is 'env'", async () => {
    const d = dir("ggenv-def-env-");
    process.env[VAR] = KEK_B64;
    const built = await buildKeyProvider({ envVar: VAR }, join(d, "vault"), join(d, "vault.key"), "env");
    expect(built.provider).toBeInstanceOf(EnvKeyProvider);
    expect(built.provider.providerId()).toBe("env");
  });

  it("falls back to the library default 'file' when type AND profile default are absent", async () => {
    const d = dir("ggenv-def-file-");
    const built = await buildKeyProvider({}, join(d, "vault"), join(d, "vault.key"));
    expect(built.provider).toBeInstanceOf(FileKeyProvider);
  });

  it("an explicit keyProvider.type always wins over the profile default", async () => {
    const d = dir("ggenv-explicit-");
    // profile default says 'env', but an explicit 'file' must be honored (no env var needed).
    const built = await buildKeyProvider({ type: "file" }, join(d, "vault"), join(d, "vault.key"), "env");
    expect(built.provider).toBeInstanceOf(FileKeyProvider);
  });

  it("selects the env provider for an explicit type 'env'", async () => {
    const d = dir("ggenv-explicit-env-");
    process.env[VAR] = KEK_B64;
    const built = await buildKeyProvider({ type: "env", envVar: VAR }, join(d, "vault"), join(d, "vault.key"));
    expect(built.provider).toBeInstanceOf(EnvKeyProvider);
  });
});
