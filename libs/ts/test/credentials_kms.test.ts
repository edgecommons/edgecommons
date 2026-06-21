/** TS credentials: KMS-via-TES key provider round trip vs floci KMS (gated by GGCOMMONS_IT_KMS). */
import { mkdtempSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

import { describe, expect, it } from "vitest";

import { openFromConfig } from "../src/credentials/config";

describe.skipIf(process.env.GGCOMMONS_IT_KMS !== "1")("kms key provider (floci)", () => {
  it("wraps + unwraps the vault DEK via KMS (put → reopen round trip)", async () => {
    process.env.AWS_ACCESS_KEY_ID ??= "test";
    process.env.AWS_SECRET_ACCESS_KEY ??= "test";
    process.env.AWS_REGION ??= "us-east-1";

    const endpoint = "http://localhost:4566";
    const kms = await import("@aws-sdk/client-kms");
    const client = new kms.KMSClient({ region: "us-east-1", endpoint });
    const created = await client.send(new kms.CreateKeyCommand({ Description: "ggcommons-ts-kms-it" }));
    const keyId = created.KeyMetadata!.KeyId!;

    const dir = mkdtempSync(join(tmpdir(), "ggvault-kms-"));
    const path = join(dir, "vault");
    const vaultCfg = { vault: { path, keyProvider: { type: "kms", kmsKeyId: keyId, region: "us-east-1", endpointUrl: endpoint } } };

    // New KMS-backed vault: DEK is KMS-wrapped at creation.
    const c1 = await openFromConfig(vaultCfg);
    c1.put("db/password", Buffer.from("s3cr3t"));
    expect(c1.getString("db/password")).toBe("s3cr3t");

    // Reopen: DEK is KMS-unwrapped from the persisted KEK and decrypts the record.
    const c2 = await openFromConfig(vaultCfg);
    expect(c2.getString("db/password")).toBe("s3cr3t");
  });
});
