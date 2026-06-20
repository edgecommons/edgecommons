import { describe, it, expect, beforeAll, afterEach } from "vitest";
import * as fs from "fs";
import * as fsp from "fs/promises";
import * as os from "os";
import * as path from "path";

import { GGCommonsBuilder, GGCommons } from "../src/ggcommons";
import { MessageBuilder } from "../src/message";
import type { ConfigurationChangeListener } from "../src/config";
import type { Config } from "../src/config/model";
import { brokerReachable, tick } from "./_fakes";

const tmp: string[] = [];
function tmpFile(name: string, contents: string): string {
  const p = path.join(os.tmpdir(), `ggc-it-${name}-${Math.random().toString(36).slice(2)}.json`);
  fs.writeFileSync(p, contents);
  tmp.push(p);
  return p;
}
afterEach(() => {
  for (const f of tmp.splice(0)) {
    try {
      fs.rmSync(f, { force: true });
    } catch {
      /* ignore */
    }
  }
});

describe("GGCommons STANDALONE lifecycle (live broker)", () => {
  let up = false;
  beforeAll(async () => {
    up = await brokerReachable();
  });

  async function build(configPath: string): Promise<GGCommons> {
    const messagingPath = tmpFile(
      "messaging",
      JSON.stringify({ messaging: { local: { host: "127.0.0.1", port: 1883, clientId: `ggc-life-${Date.now()}-${Math.random()}` } } }),
    );
    return new GGCommonsBuilder("com.example.It")
      .args(["-m", "STANDALONE", messagingPath, "-c", "FILE", configPath, "-t", "it-thing"])
      .build();
  }

  it("exposes componentName/args/config/metrics and a working messaging round-trip", async (ctx) => {
    if (!up) ctx.skip();
    const configPath = tmpFile("config", JSON.stringify({ logging: { level: "INFO" }, tags: { site: "f1" } }));
    const gg = await build(configPath);
    try {
      expect(gg.componentName()).toBe("com.example.It");
      expect(gg.args().thing).toBe("it-thing");
      expect(gg.config().thingName).toBe("it-thing");
      expect(gg.config().parsed.tags).toEqual({ site: "f1" });
      expect(gg.metrics()).toBeDefined();

      const svc = gg.messaging();
      const topic = `ggc/life/${Math.random().toString(36).slice(2)}`;
      const got: unknown[] = [];
      await svc.subscribe(topic, (_t, m) => got.push(m.getBody()));
      await svc.publish(topic, MessageBuilder.create("e", "1").withPayload({ ok: true }).build());
      for (let i = 0; i < 40 && got.length === 0; i++) await tick(50);
      expect(got).toEqual([{ ok: true }]);
    } finally {
      await gg.close();
    }
  });

  it("addConfigChangeListener fires on FILE hot reload and config() returns the new snapshot", async (ctx) => {
    if (!up) ctx.skip();
    const configPath = tmpFile("config", JSON.stringify({ logging: { level: "INFO" }, tags: { v: "1" } }));
    const gg = await build(configPath);
    try {
      const seen: Config[] = [];
      const listener: ConfigurationChangeListener = {
        onConfigurationChange(c: Config): boolean {
          seen.push(c);
          return true;
        },
      };
      gg.addConfigChangeListener(listener);

      await fsp.writeFile(configPath, JSON.stringify({ logging: { level: "DEBUG" }, tags: { v: "2" } }));
      for (let i = 0; i < 60 && seen.length === 0; i++) await tick(50);
      expect(seen.length).toBeGreaterThanOrEqual(1);
      expect(gg.config().parsed.tags).toEqual({ v: "2" });

      // removeConfigChangeListener stops further notifications.
      gg.removeConfigChangeListener(listener);
      const countAfterRemove = seen.length;
      await fsp.writeFile(configPath, JSON.stringify({ logging: { level: "INFO" }, tags: { v: "3" } }));
      for (let i = 0; i < 40 && gg.config().parsed.tags.v !== "3"; i++) await tick(50);
      expect(gg.config().parsed.tags).toEqual({ v: "3" });
      expect(seen.length).toBe(countAfterRemove);
    } finally {
      await gg.close();
    }
  });

  it("a reload that fails validation keeps the previous snapshot; a throwing listener is caught", async (ctx) => {
    if (!up) ctx.skip();
    const configPath = tmpFile("config", JSON.stringify({ tags: { v: "1" } }));
    const gg = await build(configPath);
    try {
      gg.addConfigChangeListener({
        // Async rejection: this is the path ggcommons guards with `.catch(...)`.
        async onConfigurationChange(): Promise<boolean> {
          throw new Error("listener blew up");
        },
      });
      // Invalid against the schema (target enum) -> reload rejected, snapshot unchanged.
      await fsp.writeFile(configPath, JSON.stringify({ metricEmission: { target: "not-a-real-target" } }));
      await tick(800);
      expect(gg.config().parsed.tags).toEqual({ v: "1" });

      // A valid reload triggers the throwing listener, which must be caught (no crash).
      await fsp.writeFile(configPath, JSON.stringify({ tags: { v: "2" } }));
      for (let i = 0; i < 40 && gg.config().parsed.tags.v !== "2"; i++) await tick(50);
      expect(gg.config().parsed.tags).toEqual({ v: "2" });
    } finally {
      await gg.close();
    }
  });

  it("close() resolves cleanly", async (ctx) => {
    if (!up) ctx.skip();
    const configPath = tmpFile("config", JSON.stringify({}));
    const gg = await build(configPath);
    await expect(gg.close()).resolves.toBeUndefined();
  });
});
