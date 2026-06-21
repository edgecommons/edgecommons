/** TS vault tests: functional behavior + cross-language conformance vs vault-test-vectors/. */
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

import { describe, expect, it } from "vitest";

import * as cryptoPrims from "../src/credentials/crypto";
import * as fmt from "../src/credentials/format";
import { CredentialError } from "../src/credentials/errors";
import { FileKeyProvider } from "../src/credentials/keyprovider";
import { DefaultCredentialService } from "../src/credentials/service";
import { LocalVault } from "../src/credentials/vault";
import type { SecretEntry } from "../src/credentials/format";

const VECTORS = join(__dirname, "..", "..", "..", "vault-test-vectors");

function svc(): DefaultCredentialService {
  const dir = mkdtempSync(join(tmpdir(), "ggvault-"));
  const provider = new FileKeyProvider(Buffer.alloc(32, 7));
  return new DefaultCredentialService(LocalVault.open(join(dir, "vault"), provider, 2));
}

describe("local vault", () => {
  it("put/get roundtrip and typed views", () => {
    const c = svc();
    c.put("db/password", Buffer.from("s3cr3t"));
    c.put("svc/config", Buffer.from('{"k":1}'));
    expect(c.getString("db/password")).toBe("s3cr3t");
    expect((c.getJson("svc/config") as { k: number }).k).toBe(1);
    expect(c.exists("db/password")).toBe(true);
    expect(c.get("missing")).toBeUndefined();
    expect(c.list("").map((m) => m.name)).toEqual(["db/password", "svc/config"]);
  });

  it("versions are monotonic and pruned", () => {
    const c = svc(); // keep_versions = 2
    c.put("k", Buffer.from("v1"));
    c.put("k", Buffer.from("v2"));
    c.put("k", Buffer.from("v3"));
    expect(c.versions("k")).toEqual(["00000002", "00000003"]);
    expect(c.get("k")!.asString()).toBe("v3");
    expect(c.getVersion("k", "00000002")!.asString()).toBe("v2");
    expect(c.getVersion("k", "00000001")).toBeUndefined();
  });

  it("persists and reopens with the same key", () => {
    const dir = mkdtempSync(join(tmpdir(), "ggvault-"));
    const p = new FileKeyProvider(Buffer.alloc(32, 7));
    new DefaultCredentialService(LocalVault.open(join(dir, "vault"), p, 2)).put("token", Buffer.from("abc"));
    const reopened = new DefaultCredentialService(LocalVault.open(join(dir, "vault"), p, 2));
    expect(reopened.getString("token")).toBe("abc");
  });

  it("wrong KEK fails closed", () => {
    const dir = mkdtempSync(join(tmpdir(), "ggvault-"));
    new DefaultCredentialService(
      LocalVault.open(join(dir, "vault"), new FileKeyProvider(Buffer.alloc(32, 7)), 2),
    ).put("token", Buffer.from("abc"));
    expect(() => LocalVault.open(join(dir, "vault"), new FileKeyProvider(Buffer.alloc(32, 9)), 2)).toThrow(
      CredentialError,
    );
  });

  it("tamper is detected", () => {
    const dir = mkdtempSync(join(tmpdir(), "ggvault-"));
    const path = join(dir, "vault");
    new DefaultCredentialService(
      LocalVault.open(path, new FileKeyProvider(Buffer.alloc(32, 7)), 2),
    ).put("k", Buffer.from("v1"));
    const vf = JSON.parse(readFileSync(path, "utf8"));
    const ct = Buffer.from(vf.secrets.k.versions[0].ciphertext, "base64");
    ct[0] ^= 1;
    vf.secrets.k.versions[0].ciphertext = ct.toString("base64");
    writeFileSync(path, JSON.stringify(vf));
    expect(() => LocalVault.open(path, new FileKeyProvider(Buffer.alloc(32, 7)), 2)).toThrow(CredentialError);
  });
});

describe.skipIf(!existsSync(join(VECTORS, "vault.json")))("cross-language conformance", () => {
  it("decrypts the canonical vault and reproduces ciphertext/wrappedDek/MAC", () => {
    const vec = JSON.parse(readFileSync(join(VECTORS, "vectors.json"), "utf8"));
    const kek = Buffer.from(vec.kekB64, "base64");
    const dek = Buffer.from(vec.dekB64, "base64");
    const vaultId: string = vec.vaultId;

    // (1) decrypt the Rust-generated canonical vault using the committed key file
    const provider = FileKeyProvider.fromKeyFile(join(VECTORS, "vault.key"));
    const v = LocalVault.open(join(VECTORS, "vault.json"), provider, 2);
    expect(v.get("alpha")!.bytes().toString("utf8")).toBe("hello");
    expect((v.get("beta")!.asJson() as { x: number }).x).toBe(1);

    // (2) reproduce the wrapped DEK
    const wrapped = cryptoPrims.seal(kek, Buffer.from(vec.wrapNonceB64, "base64"), fmt.dekWrapAad(vaultId), dek);
    expect(wrapped.toString("base64")).toBe(vec.wrappedDekB64);

    // (3) reproduce each record ciphertext and build the secrets map for the MAC
    const secrets: Record<string, SecretEntry> = {};
    for (const r of vec.records) {
      const nonce = Buffer.from(r.nonceB64, "base64");
      const pt = Buffer.from(r.plaintextB64, "base64");
      const ct = cryptoPrims.seal(dek, nonce, fmt.recordAad(vaultId, r.name, r.version), pt);
      expect(ct.toString("base64")).toBe(r.ciphertextB64);
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

    // (4) reproduce the MAC over the canonical byte string
    const macKey = cryptoPrims.deriveMacKey(dek, vaultId);
    const mac = cryptoPrims
      .hmacSha256(macKey, fmt.macInput(vaultId, secrets, (s) => Buffer.from(s, "base64")))
      .toString("base64");
    expect(mac).toBe(vec.macB64);
  });
});
