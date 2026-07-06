/** TS credentials Phase 2: namespacing isolation + central sync vs floci secretsmanager,
 * plus a mocked-SDK unit suite for {@link AwsSecretsManagerSource}. */
import { randomUUID } from "crypto";
import { mkdtempSync, readFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { openFromConfig } from "../src/credentials/config";
import { CredentialError } from "../src/credentials/errors";
import { FileKeyProvider } from "../src/credentials/keyprovider";
import { DefaultCredentialService } from "../src/credentials/service";
import { LocalVault } from "../src/credentials/vault";

/**
 * Mock of `@aws-sdk/client-secrets-manager`. `AwsSecretsManagerSource.create` dynamically imports
 * this package, so a module-level `vi.mock` factory intercepts it without hitting AWS. The
 * `GetSecretValueCommand` constructor just stashes its input so the mocked `send` can read `SecretId`.
 */
const sendMock = vi.fn();
vi.mock("@aws-sdk/client-secrets-manager", () => ({
  SecretsManagerClient: class {
    public readonly config: unknown;
    constructor(cfg: unknown) {
      this.config = cfg;
    }
    send = sendMock;
  },
  GetSecretValueCommand: class {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    constructor(public readonly input: any) {}
  },
}));

// Imported after the mock declaration; vitest hoists vi.mock above imports regardless.
import { AwsSecretsManagerSource } from "../src/credentials/central";

describe("namespacing", () => {
  it("isolates components in a shared vault", () => {
    const dir = mkdtempSync(join(tmpdir(), "ggvault-ns-"));
    const path = join(dir, "vault");
    const kek = Buffer.alloc(32, 5);
    const c1 = new DefaultCredentialService(LocalVault.open(path, new FileKeyProvider(kek), 2), "thing-1/CompA");
    const c2 = new DefaultCredentialService(LocalVault.open(path, new FileKeyProvider(kek), 2), "thing-1/CompB");

    c1.put("db/password", Buffer.from("a-secret"));
    c2.put("db/password", Buffer.from("b-secret"));
    expect(c1.getString("db/password")).toBe("a-secret");
    expect(c2.getString("db/password")).toBe("b-secret");
    expect(c1.list("").map((m) => m.name)).toEqual(["db/password"]);

    const raw = readFileSync(path, "utf8");
    expect(raw).toContain("thing-1/CompA/db/password");
    expect(raw).toContain("thing-1/CompB/db/password");
  });
});

describe("AwsSecretsManagerSource (mocked SDK)", () => {
  beforeEach(() => sendMock.mockReset());
  afterEach(() => vi.clearAllMocks());

  it("fetches a string secret and maps SecretString + VersionId", async () => {
    sendMock.mockResolvedValueOnce({ SecretString: "hello", VersionId: "v-abc" });
    const src = await AwsSecretsManagerSource.create("us-east-1", "http://localhost:4566");
    const r = await src.fetch("my/secret");
    expect(r).toBeDefined();
    expect(r!.bytes.toString("utf-8")).toBe("hello");
    expect(r!.centralVersionId).toBe("v-abc");
    expect(r!.labels).toEqual({});
    // the GetSecretValueCommand carried the right SecretId
    const cmd = sendMock.mock.calls[0][0] as { input: { SecretId: string } };
    expect(cmd.input.SecretId).toBe("my/secret");
  });

  it("fetches a binary secret from SecretBinary", async () => {
    const blob = new Uint8Array([1, 2, 3, 250]);
    sendMock.mockResolvedValueOnce({ SecretBinary: blob, VersionId: "v-bin" });
    const src = await AwsSecretsManagerSource.create();
    const r = await src.fetch("bin");
    expect(Array.from(r!.bytes)).toEqual([1, 2, 3, 250]);
    expect(r!.centralVersionId).toBe("v-bin");
  });

  it("defaults centralVersionId to '' when VersionId is absent", async () => {
    sendMock.mockResolvedValueOnce({ SecretString: "x" });
    const src = await AwsSecretsManagerSource.create();
    const r = await src.fetch("x");
    expect(r!.centralVersionId).toBe("");
  });

  it("returns undefined when the response has neither SecretString nor SecretBinary", async () => {
    sendMock.mockResolvedValueOnce({ VersionId: "v-empty" });
    const src = await AwsSecretsManagerSource.create();
    expect(await src.fetch("empty")).toBeUndefined();
  });

  it("returns undefined on ResourceNotFoundException (offline-first miss, not an error)", async () => {
    sendMock.mockRejectedValueOnce(Object.assign(new Error("nope"), { name: "ResourceNotFoundException" }));
    const src = await AwsSecretsManagerSource.create();
    expect(await src.fetch("missing")).toBeUndefined();
  });

  it("wraps any other SDK error in a CredentialError including the secret name", async () => {
    sendMock.mockRejectedValueOnce(Object.assign(new Error("access denied"), { name: "AccessDeniedException" }));
    const src = await AwsSecretsManagerSource.create();
    const err = await src.fetch("locked").catch((e) => e);
    expect(err).toBeInstanceOf(CredentialError);
    expect((err as Error).message).toMatch(/locked/);
    expect((err as Error).message).toMatch(/access denied/);
  });

  it("handles a non-Error rejection by stringifying it", async () => {
    sendMock.mockRejectedValueOnce("boom-string");
    const src = await AwsSecretsManagerSource.create();
    const err = await src.fetch("s").catch((e) => e);
    expect(err).toBeInstanceOf(CredentialError);
    expect((err as Error).message).toMatch(/boom-string/);
  });
});

describe.skipIf(process.env.EDGECOMMONS_IT_SM !== "1")("central sync (floci)", () => {
  it("bootstrap + rotation + no-churn from Secrets Manager", async () => {
    process.env.AWS_ACCESS_KEY_ID ??= "test";
    process.env.AWS_SECRET_ACCESS_KEY ??= "test";
    process.env.AWS_REGION ??= "us-east-1";
    const sm = await import("@aws-sdk/client-secrets-manager");
    const client = new sm.SecretsManagerClient({ region: "us-east-1", endpoint: "http://localhost:4566" });
    const name = `edgecommons-ts-cred-${randomUUID()}`;
    await client.send(new sm.CreateSecretCommand({ Name: name, SecretString: "v1" }));
    try {
      const dir = mkdtempSync(join(tmpdir(), "ggvault-sm-"));
      const creds = await openFromConfig({
        vault: { path: join(dir, "vault"), keyProvider: { type: "file", keyPath: join(dir, "vault.key") } },
        central: {
          type: "awsSecretsManager", region: "us-east-1", endpointUrl: "http://localhost:4566",
          bootstrapOnStart: true, refreshIntervalSecs: 0, sync: { secrets: [name] },
        },
      }); // namespace "" → central id == local key == name

      expect(creds.getString(name)).toBe("v1");

      await client.send(new sm.PutSecretValueCommand({ SecretId: name, SecretString: "v2" }));
      await creds.refresh();
      expect(creds.getString(name)).toBe("v2");
      expect(creds.versions(name).length).toBeGreaterThanOrEqual(2); // previous version retained

      const before = creds.versions(name).length;
      await creds.refresh();
      expect(creds.versions(name).length).toBe(before); // no churn when unchanged
    } finally {
      await client.send(new sm.DeleteSecretCommand({ SecretId: name, ForceDeleteWithoutRecovery: true }));
    }
  });
});
