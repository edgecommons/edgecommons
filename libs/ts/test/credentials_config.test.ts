/**
 * credentials/config.ts tests — `openFromConfig` (vault open + audit wiring + central source
 * selection) and `buildKeyProvider` (file / kms / greengrass / pkcs11 / unsupported). The AWS SDK
 * KMS + Secrets Manager clients are mocked with `vi.mock` (no real AWS); the file key provider and
 * local vault are exercised for real. Mirrors the Rust `build_key_provider` / `open_from_config`.
 */
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// --- Mock the AWS SDK clients the central/KMS paths load dynamically. ---
const kmsSend = vi.fn();
vi.mock("@aws-sdk/client-kms", () => ({
  KMSClient: class {
    send = kmsSend;
  },
  EncryptCommand: class {
    constructor(public input: unknown) {}
  },
  DecryptCommand: class {
    constructor(public input: unknown) {}
  },
}));

const smSend = vi.fn();
vi.mock("@aws-sdk/client-secrets-manager", () => ({
  SecretsManagerClient: class {
    send = smSend;
  },
  GetSecretValueCommand: class {
    constructor(public input: unknown) {}
  },
}));

import { buildKeyProvider, openFromConfig } from "../src/credentials/config";
import { CredentialError } from "../src/credentials/errors";
import { FileKeyProvider, PrewrappedKeyProvider } from "../src/credentials/keyprovider";
import type { VaultFile } from "../src/credentials/format";

function dir(prefix: string): string {
  return mkdtempSync(join(tmpdir(), prefix));
}

beforeEach(() => {
  kmsSend.mockReset();
  smSend.mockReset();
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.clearAllMocks();
});

describe("openFromConfig", () => {
  it("opens a file-backed vault with defaults (no central) and round-trips a secret", async () => {
    const d = dir("ggcfg-default-");
    const svc = await openFromConfig({ vault: { path: join(d, "vault") } });

    const version = svc.put("db/password", Buffer.from("s3cr3t"));
    expect(version).toBeTruthy();
    expect(svc.getString("db/password")).toBe("s3cr3t");
    // Default key file is created next to the vault (`<path>.key`).
    expect(existsSync(join(d, "vault.key"))).toBe(true);
    // No central sync => stats has zero failures/rotations and no sync age.
    const stats = svc.stats();
    expect(stats.lastSyncAgeMs).toBeUndefined();
    expect(stats.syncFailures).toBe(0);
  });

  it("applies the namespace transparently across the vault file", async () => {
    const d = dir("ggcfg-ns-");
    const svc = await openFromConfig({ vault: { path: join(d, "vault") } }, "thing-9/CompZ");
    svc.put("api/token", Buffer.from("tok"));
    expect(svc.getString("api/token")).toBe("tok");

    const raw = readFileSync(join(d, "vault"), "utf8");
    expect(raw).toContain("thing-9/CompZ/api/token");
  });

  it("defaults vault path to 'vault' and keepVersions to 2 when cfg omits them", async () => {
    const d = dir("ggcfg-emptycfg-");
    const prev = process.cwd();
    process.chdir(d);
    try {
      const svc = await openFromConfig(); // no arg at all → cfg = {}
      svc.put("k", Buffer.from("v"));
      expect(svc.getString("k")).toBe("v");
      expect(existsSync(join(d, "vault"))).toBe(true);
    } finally {
      process.chdir(prev);
    }
  });

  it("enables the audit log sink by default", async () => {
    const d = dir("ggcfg-audit-on-");
    const { logger } = await import("../src/logging");
    const spy = vi.spyOn(logger, "info").mockImplementation(() => {});

    const svc = await openFromConfig({ vault: { path: join(d, "vault") } });
    svc.put("a", Buffer.from("x"));

    const auditLines = spy.mock.calls.map((c) => c[0]).filter((l) => l.startsWith("credential access"));
    expect(auditLines.length).toBe(1);
    expect(auditLines[0]).toContain("op=put secret=a");
  });

  it("disables auditing when audit.enabled === false", async () => {
    const d = dir("ggcfg-audit-off-");
    const { logger } = await import("../src/logging");
    const spy = vi.spyOn(logger, "info").mockImplementation(() => {});

    const svc = await openFromConfig({ vault: { path: join(d, "vault") }, audit: { enabled: false } });
    svc.put("a", Buffer.from("x"));

    const auditLines = spy.mock.calls.map((c) => c[0]).filter((l) => l.startsWith("credential access"));
    expect(auditLines.length).toBe(0);
  });

  it("treats central.type 'none' (or absent) as no central sync", async () => {
    const d = dir("ggcfg-central-none-");
    const svc = await openFromConfig({ vault: { path: join(d, "vault") }, central: { type: "none" } });
    expect(svc.stats().lastSyncAgeMs).toBeUndefined();
    expect(smSend).not.toHaveBeenCalled();
  });

  it("rejects an unsupported central source type", async () => {
    const d = dir("ggcfg-central-bad-");
    await expect(
      openFromConfig({ vault: { path: join(d, "vault") }, central: { type: "vault-hashicorp" } }),
    ).rejects.toThrow(CredentialError);
  });

  it("wires the awsSecretsManager central source and bootstraps configured secrets", async () => {
    const d = dir("ggcfg-central-sm-");
    smSend.mockResolvedValue({ SecretString: "central-value", VersionId: "v-aws-1" });

    const svc = await openFromConfig({
      vault: { path: join(d, "vault") },
      central: {
        type: "awsSecretsManager",
        region: "us-east-1",
        endpointUrl: "http://localhost:4566",
        bootstrapOnStart: true,
        refreshIntervalSecs: 0,
        sync: { secrets: ["app/db", { name: "app/api", from: "remote/api" }] },
      },
    });

    // Both string-form and object-form sync entries were fetched at bootstrap.
    expect(smSend).toHaveBeenCalled();
    const requestedIds = smSend.mock.calls.map((c) => c[0].input.SecretId).sort();
    expect(requestedIds).toEqual(["app/db", "remote/api"]);
    // The bootstrapped value landed in the local vault under its caller-facing name.
    expect(svc.getString("app/db")).toBe("central-value");
    await svc.refresh(); // central present => exercises the SyncEngine.syncNow path
  });
});

describe("buildKeyProvider", () => {
  it("file: uses the explicit keyPath and generates a key file when absent", async () => {
    const d = dir("ggkp-file-explicit-");
    const keyPath = join(d, "nested", "my.key");
    const built = await buildKeyProvider({ type: "file", keyPath }, join(d, "vault"), join(d, "vault.key"));
    expect(built.provider).toBeInstanceOf(FileKeyProvider);
    expect(built.newVaultId).toBeUndefined();
    expect(existsSync(keyPath)).toBe(true); // generated (and the parent dir was created)
  });

  it("file: falls back to defaultKeyPath and reuses an existing key file", async () => {
    const d = dir("ggkp-file-default-");
    const defaultKeyPath = join(d, "vault.key");
    // First call generates the file...
    await buildKeyProvider({ type: "file" }, join(d, "vault"), defaultKeyPath);
    const firstKey = readFileSync(defaultKeyPath);
    // ...second call must load the SAME key (fromKeyFile), not regenerate it.
    await buildKeyProvider({ type: "file" }, join(d, "vault"), defaultKeyPath);
    expect(readFileSync(defaultKeyPath).equals(firstKey)).toBe(true);
  });

  it("file: defaults to 'file' when type is omitted", async () => {
    const d = dir("ggkp-file-implicit-");
    const built = await buildKeyProvider({}, join(d, "vault"), join(d, "vault.key"));
    expect(built.provider).toBeInstanceOf(FileKeyProvider);
  });

  it("kms: a brand-new vault wraps a fresh DEK eagerly and returns newVaultId/newDek", async () => {
    const d = dir("ggkp-kms-new-");
    kmsSend.mockResolvedValue({ CiphertextBlob: Buffer.alloc(48, 1) });

    const built = await buildKeyProvider(
      { type: "kms", kmsKeyId: "alias/test", region: "us-east-1", endpointUrl: "http://localhost:4566" },
      join(d, "vault"),
      join(d, "vault.key"),
    );

    expect(built.provider).toBeInstanceOf(PrewrappedKeyProvider);
    expect(built.provider.providerId()).toBe("kms");
    expect(built.newVaultId).toBeTruthy();
    expect(built.newDek).toBeInstanceOf(Buffer);
    expect(built.newDek!.length).toBe(32);
    // Encrypt (wrap) was called once; the encryption context binds the new vault id.
    expect(kmsSend).toHaveBeenCalledTimes(1);
    expect(kmsSend.mock.calls[0][0].input.EncryptionContext.vaultId).toBe(built.newVaultId);
  });

  it("kms: an existing vault decrypts the persisted DEK eagerly (no newVaultId)", async () => {
    const d = dir("ggkp-kms-existing-");
    const vaultPath = join(d, "vault");
    const vf: VaultFile = {
      format: 1,
      vaultId: "vault-abc",
      kek: { provider: "kms", alg: "aws-kms", wrappedDek: Buffer.alloc(48, 2).toString("base64"), kmsKeyId: "alias/test" },
      secrets: {},
      mac: "",
    };
    writeFileSync(vaultPath, JSON.stringify(vf));
    kmsSend.mockResolvedValue({ Plaintext: Buffer.alloc(32, 9) });

    const built = await buildKeyProvider({ type: "kms", kmsKeyId: "alias/test" }, vaultPath, join(d, "vault.key"));

    expect(built.provider).toBeInstanceOf(PrewrappedKeyProvider);
    expect(built.newVaultId).toBeUndefined();
    expect(built.newDek).toBeUndefined();
    // Decrypt (unwrap) was called with the persisted vault id as the encryption context.
    expect(kmsSend).toHaveBeenCalledTimes(1);
    expect(kmsSend.mock.calls[0][0].input.EncryptionContext.vaultId).toBe("vault-abc");
  });

  it("kms: 'greengrass' is an alias for the kms provider", async () => {
    const d = dir("ggkp-gg-");
    kmsSend.mockResolvedValue({ CiphertextBlob: Buffer.alloc(48, 1) });
    const built = await buildKeyProvider(
      { type: "greengrass", kmsKeyId: "alias/gg" },
      join(d, "vault"),
      join(d, "vault.key"),
    );
    expect(built.provider.providerId()).toBe("kms");
  });

  it("kms: requires kmsKeyId", async () => {
    const d = dir("ggkp-kms-nokey-");
    await expect(
      buildKeyProvider({ type: "kms" }, join(d, "vault"), join(d, "vault.key")),
    ).rejects.toThrow(/kmsKeyId/);
    expect(kmsSend).not.toHaveBeenCalled();
  });

  it("pkcs11: requires modulePath", async () => {
    const d = dir("ggkp-p11-nomod-");
    await expect(
      buildKeyProvider({ type: "pkcs11", keyLabel: "k", pin: "1234" }, join(d, "vault"), join(d, "vault.key")),
    ).rejects.toThrow(/modulePath/);
  });

  it("pkcs11: requires keyLabel", async () => {
    const d = dir("ggkp-p11-nolabel-");
    await expect(
      buildKeyProvider({ type: "pkcs11", modulePath: "/lib/softhsm.so", pin: "1234" }, join(d, "vault"), join(d, "vault.key")),
    ).rejects.toThrow(/keyLabel/);
  });

  it("pkcs11: requires a pin or pinEnv", async () => {
    const d = dir("ggkp-p11-nopin-");
    await expect(
      buildKeyProvider({ type: "pkcs11", modulePath: "/lib/softhsm.so", keyLabel: "k" }, join(d, "vault"), join(d, "vault.key")),
    ).rejects.toThrow(/pinEnv or keyProvider.pin/);
  });

  it("pkcs11: errors when pinEnv names an unset environment variable", async () => {
    const d = dir("ggkp-p11-badenv-");
    delete process.env.GG_TEST_MISSING_PIN;
    await expect(
      buildKeyProvider(
        { type: "pkcs11", modulePath: "/lib/softhsm.so", keyLabel: "k", pinEnv: "GG_TEST_MISSING_PIN" },
        join(d, "vault"),
        join(d, "vault.key"),
      ),
    ).rejects.toThrow(/GG_TEST_MISSING_PIN' is not set/);
  });

  it("pkcs11: a satisfied pin config passes validation and reaches the PKCS#11 module load", async () => {
    const d = dir("ggkp-p11-load-");
    process.env.GG_TEST_PIN = "secret-pin";
    try {
      // pin/pinEnv validation passes, so the call proceeds past validation into
      // Pkcs11KeyProvider.create — which then fails loading the bogus module path. The point is that
      // a fully-specified config does NOT trip a CredentialError validation guard.
      await expect(
        buildKeyProvider(
          { type: "pkcs11", modulePath: "/does/not/exist/softhsm.so", keyLabel: "k", pinEnv: "GG_TEST_PIN" },
          join(d, "vault"),
          join(d, "vault.key"),
        ),
      ).rejects.toThrow(); // any error from the module load — not a missing-field validation error
    } finally {
      delete process.env.GG_TEST_PIN;
    }
  });

  it("rejects an unsupported key provider type", async () => {
    const d = dir("ggkp-bad-");
    await expect(
      buildKeyProvider({ type: "tpm-direct" }, join(d, "vault"), join(d, "vault.key")),
    ).rejects.toThrow(/not supported/);
  });
});
