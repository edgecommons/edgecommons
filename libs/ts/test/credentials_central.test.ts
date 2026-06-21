/** TS credentials Phase 2: namespacing isolation + central sync vs floci secretsmanager. */
import { randomUUID } from "crypto";
import { mkdtempSync, readFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

import { describe, expect, it } from "vitest";

import { openFromConfig } from "../src/credentials/config";
import { FileKeyProvider } from "../src/credentials/keyprovider";
import { DefaultCredentialService } from "../src/credentials/service";
import { LocalVault } from "../src/credentials/vault";

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

describe.skipIf(process.env.GGCOMMONS_IT_SM !== "1")("central sync (floci)", () => {
  it("bootstrap + rotation + no-churn from Secrets Manager", async () => {
    process.env.AWS_ACCESS_KEY_ID ??= "test";
    process.env.AWS_SECRET_ACCESS_KEY ??= "test";
    process.env.AWS_REGION ??= "us-east-1";
    const sm = await import("@aws-sdk/client-secrets-manager");
    const client = new sm.SecretsManagerClient({ region: "us-east-1", endpoint: "http://localhost:4566" });
    const name = `ggcommons-ts-cred-${randomUUID()}`;
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
