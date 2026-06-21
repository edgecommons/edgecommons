/**
 * TS credentials: PKCS#11 (HSM/TPM/SoftHSM) key provider round trip (gated by GGCOMMONS_IT_PKCS11).
 * Env: PKCS11_MODULE, PKCS11_TOKEN, PKCS11_KEY, PKCS11_PIN (and SOFTHSM2_CONF for SoftHSM).
 */
import { mkdtempSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

import { describe, expect, it } from "vitest";

import { openFromConfig } from "../src/credentials/config";

describe.skipIf(process.env.GGCOMMONS_IT_PKCS11 !== "1")("pkcs11 key provider (SoftHSM)", () => {
  it("wraps + unwraps the vault DEK on the token (put → reopen round trip)", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ggvault-p11-"));
    const path = join(dir, "vault");
    const vaultCfg = {
      vault: {
        path,
        keyProvider: {
          type: "pkcs11",
          modulePath: process.env.PKCS11_MODULE!,
          tokenLabel: process.env.PKCS11_TOKEN!,
          keyLabel: process.env.PKCS11_KEY!,
          pin: process.env.PKCS11_PIN!,
        },
      },
    };

    // New pkcs11-backed vault: DEK is HSM-wrapped at creation.
    const c1 = await openFromConfig(vaultCfg);
    c1.put("db/password", Buffer.from("s3cr3t"));
    expect(c1.getString("db/password")).toBe("s3cr3t");

    // Reopen: DEK is HSM-unwrapped from the persisted KEK and decrypts the record.
    const c2 = await openFromConfig(vaultCfg);
    expect(c2.getString("db/password")).toBe("s3cr3t");
  });
});
