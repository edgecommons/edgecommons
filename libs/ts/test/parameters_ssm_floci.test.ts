/**
 * End-to-end awsSsm source vs a local AWS emulator (floci/LocalStack SSM on :4566).
 *
 * Exercises the real `@aws-sdk/client-ssm` path (excluded from unit coverage in ssm.ts): seed a
 * String, a SecureString and a 2-key tree, then read them back via the source under test and
 * assert values, the secure flag, version, missing->undefined, and get-by-path. Skips when no
 * emulator is reachable. Override the endpoint with GGCOMMONS_SSM_ENDPOINT.
 */
import { describe, it, expect, beforeAll, afterAll } from "vitest";
import * as net from "node:net";

import { AwsSsmSource } from "../src/parameters/ssm";

const ENDPOINT = process.env.GGCOMMONS_SSM_ENDPOINT ?? "http://localhost:4566";

function reachable(endpoint: string, timeoutMs = 2000): Promise<boolean> {
  const u = new URL(endpoint);
  return new Promise((resolve) => {
    const sock = net.connect({ host: u.hostname, port: Number(u.port || "4566") });
    const done = (ok: boolean) => {
      sock.destroy();
      resolve(ok);
    };
    sock.setTimeout(timeoutMs);
    sock.once("connect", () => done(true));
    sock.once("timeout", () => done(false));
    sock.once("error", () => done(false));
  });
}

describe("awsSsm source vs local AWS emulator (floci/LocalStack SSM :4566)", () => {
  let up = false;
  const prefix = `/ggcommons-it-ts-${process.pid}`;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let admin: any;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let mod: any;

  beforeAll(async () => {
    up = await reachable(ENDPOINT);
    if (!up) return;
    process.env.AWS_ACCESS_KEY_ID ??= "test";
    process.env.AWS_SECRET_ACCESS_KEY ??= "test";
    process.env.AWS_DEFAULT_REGION ??= "us-east-1";
    mod = await import("@aws-sdk/client-ssm");
    admin = new mod.SSMClient({ region: "us-east-1", endpoint: ENDPOINT });
    const put = (name: string, value: string, type: string) =>
      admin.send(new mod.PutParameterCommand({ Name: name, Value: value, Type: type, Overwrite: true }));
    await put(`${prefix}/plain`, "us-east-1", "String");
    await put(`${prefix}/secure`, "p@ss", "SecureString");
    await put(`${prefix}/tree/a`, "1", "String");
    await put(`${prefix}/tree/b`, "2", "String");
  });

  afterAll(async () => {
    if (!up || !admin) return;
    for (const suffix of ["/plain", "/secure", "/tree/a", "/tree/b"]) {
      try {
        await admin.send(new mod.DeleteParameterCommand({ Name: `${prefix}${suffix}` }));
      } catch {
        /* best-effort cleanup */
      }
    }
  });

  it("reads String, SecureString (decrypted), missing->undefined, and by-path", async (ctx) => {
    if (!up) ctx.skip();
    const src = await AwsSsmSource.create("us-east-1", ENDPOINT, true);

    const plain = await src.fetch(`${prefix}/plain`);
    expect(plain?.value.toString("utf-8")).toBe("us-east-1");
    expect(plain?.secure).toBe(false);
    expect(plain?.version).toBeDefined();

    const secure = await src.fetch(`${prefix}/secure`);
    expect(secure?.value.toString("utf-8")).toBe("p@ss"); // decrypted
    expect(secure?.secure).toBe(true);

    expect(await src.fetch(`${prefix}/missing`)).toBeUndefined();

    const tree = Object.fromEntries(
      (await src.fetchByPath(`${prefix}/tree`, true)).map(([k, v]) => [k, v.value.toString("utf-8")]),
    );
    expect(tree[`${prefix}/tree/a`]).toBe("1");
    expect(tree[`${prefix}/tree/b`]).toBe("2");
  });
});
