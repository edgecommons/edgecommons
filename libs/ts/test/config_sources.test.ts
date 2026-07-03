import { describe, it, expect, afterEach, vi } from "vitest";
import * as fs from "fs";
import * as fsp from "fs/promises";
import * as os from "os";
import * as path from "path";

import { GgError } from "../src/errors";
import { FileConfigSource } from "../src/config/source/file";
import { EnvConfigSource } from "../src/config/source/env";
import { ConfigComponentSource } from "../src/config/source/config_component";
import { GreengrassConfigSource } from "../src/config/source/greengrass";
import { ShadowConfigSource } from "../src/config/source/shadow";
import { buildConfigSource } from "../src/config/source";
import { IpcMessagingProvider } from "../src/messaging/ipc-provider";
import { greengrasscoreipc } from "aws-iot-device-sdk-v2";

import { RecordingMessagingService, FakeIpcClient, tick } from "./_fakes";

import model = greengrasscoreipc.model;

const tmp: string[] = [];
function tmpFile(contents: string): string {
  const p = path.join(os.tmpdir(), `ggc-cfg-${process.pid}-${Math.random().toString(36).slice(2)}.json`);
  fs.writeFileSync(p, contents);
  tmp.push(p);
  return p;
}
function ipcWith(client: FakeIpcClient): IpcMessagingProvider {
  return IpcMessagingProvider._withClient(
    client as unknown as greengrasscoreipc.Client,
    model.ReceiveMode.RECEIVE_MESSAGES_FROM_OTHERS,
  );
}

afterEach(() => {
  for (const f of tmp.splice(0)) {
    try {
      fs.rmSync(f, { force: true });
    } catch {
      /* ignore */
    }
  }
  vi.restoreAllMocks();
});

describe("FileConfigSource", () => {
  it("load() parses a JSON file", async () => {
    const p = tmpFile(JSON.stringify({ a: 1, b: "x" }));
    const src = new FileConfigSource(p);
    expect(src.sourceName()).toBe("FILE");
    expect(await src.load()).toEqual({ a: 1, b: "x" });
  });

  it("load() throws GgError(Io) when the file is missing", async () => {
    const src = new FileConfigSource(path.join(os.tmpdir(), "does-not-exist-xyz.json"));
    await expect(src.load()).rejects.toBeInstanceOf(GgError);
    await src.load().catch((e) => expect((e as GgError).kind).toBe("Io"));
  });

  it("load() throws GgError(Config) on a parse error", async () => {
    const p = tmpFile("{ not json");
    const src = new FileConfigSource(p);
    await src.load().catch((e) => expect((e as GgError).kind).toBe("Config"));
    await expect(src.load()).rejects.toBeInstanceOf(GgError);
  });

  it("watch() hot-reloads on a valid write and skips a malformed write", async () => {
    const p = tmpFile(JSON.stringify({ v: 1 }));
    const src = new FileConfigSource(p);
    const updates: unknown[] = [];
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const watch = await src.watch((doc) => updates.push(doc));
    expect(watch).toBeDefined();

    // Valid write -> onUpdate fires with the new doc.
    await fsp.writeFile(p, JSON.stringify({ v: 2 }));
    for (let i = 0; i < 40 && updates.length === 0; i++) await tick(50);
    expect(updates.length).toBeGreaterThanOrEqual(1);
    expect(updates[updates.length - 1]).toEqual({ v: 2 });

    // Malformed write -> NOT delivered (previous valid doc stays in effect) and is warned.
    // Note: fs.watch coalesces poorly on Linux (inotify can fire >1 event per write), so a
    // valid write may re-deliver the SAME doc — assert by content/intent, not an exact count.
    await fsp.writeFile(p, "{ broken");
    await tick(300);
    expect(updates[updates.length - 1]).toEqual({ v: 2 });
    expect(warn).toHaveBeenCalled();
    expect(updates.some((u) => JSON.stringify(u) === "{ broken")).toBe(false);

    await watch!.close();
    // After close, a fresh write delivers no NEW doc ({ v: 3 } never appears).
    await fsp.writeFile(p, JSON.stringify({ v: 3 }));
    await tick(300);
    expect(updates.some((u) => JSON.stringify(u) === JSON.stringify({ v: 3 }))).toBe(false);
  });

  it("watch() returns undefined when the directory cannot be watched", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    // A path under a non-existent directory: fs.watch on the missing parent throws.
    const missing = path.join(os.tmpdir(), `ggc-nodir-${Math.random().toString(36).slice(2)}`, "config.json");
    const src = new FileConfigSource(missing);
    const watch = await src.watch(() => undefined);
    expect(watch).toBeUndefined();
    expect(warn).toHaveBeenCalled();
  });
});

describe("EnvConfigSource", () => {
  it("load() parses the env var", async () => {
    process.env.GGC_TEST_CFG = JSON.stringify({ x: 9 });
    const src = new EnvConfigSource("GGC_TEST_CFG");
    expect(src.sourceName()).toBe("ENV");
    expect(await src.load()).toEqual({ x: 9 });
    delete process.env.GGC_TEST_CFG;
  });

  it("load() throws GgError(Config) when unset", async () => {
    delete process.env.GGC_UNSET_VAR;
    const src = new EnvConfigSource("GGC_UNSET_VAR");
    await expect(src.load()).rejects.toBeInstanceOf(GgError);
    await src.load().catch((e) => expect((e as GgError).kind).toBe("Config"));
  });

  it("load() throws GgError(Json) on invalid JSON", async () => {
    process.env.GGC_BAD = "not json";
    const src = new EnvConfigSource("GGC_BAD");
    await src.load().catch((e) => expect((e as GgError).kind).toBe("Json"));
    delete process.env.GGC_BAD;
  });

  it("watch() returns undefined (no hot reload)", async () => {
    const src = new EnvConfigSource("CONFIG");
    expect(await src.watch(() => undefined)).toBeUndefined();
  });
});

describe("ConfigComponentSource", () => {
  it("load() requests the UNS Flow-A rendezvous, self-identifies in the body, and returns the reply body", async () => {
    const svc = new RecordingMessagingService();
    svc.replyBody = { from: "config-component" };
    let requestTopic: string | undefined;
    let requestBody: unknown;
    const origReq = svc.request.bind(svc);
    svc.request = ((topic, msg, timeoutMs) => {
      requestTopic = topic;
      requestBody = msg.getBody();
      // The bootstrap request carries NO identity (pre-config, §1.5).
      expect(msg.getIdentity()).toBeUndefined();
      return origReq(topic, msg, timeoutMs);
    }) as typeof svc.request;
    const src = new ConfigComponentSource(svc, "thing-A", "com.example.C");
    expect(src.sourceName()).toBe("CONFIG_COMPONENT");
    const loaded = await src.load();
    expect(loaded).toEqual({ from: "config-component" });
    // D-U19 Flow A: server rendezvous under the logical component name `config`; the requester
    // self-identifies with the sanitized SHORT component name in the body.
    expect(requestTopic).toBe("ecv1/thing-A/config/main/cmd/get-configuration");
    expect(requestBody).toEqual({ component: "C" });
  });

  it("sanitizes the thing/component tokens into the minted topics", async () => {
    const svc = new RecordingMessagingService();
    svc.replyBody = {};
    let requestTopic: string | undefined;
    const origReq = svc.request.bind(svc);
    svc.request = ((topic, msg, timeoutMs) => {
      requestTopic = topic;
      return origReq(topic, msg, timeoutMs);
    }) as typeof svc.request;
    const src = new ConfigComponentSource(svc, "thing/A+B", "com.example.My#Comp");
    await src.load();
    expect(requestTopic).toBe("ecv1/thing_A_B/config/main/cmd/get-configuration");
    const watch = await src.watch(() => undefined);
    expect(svc.subscriptions.has("ecv1/thing_A_B/My_Comp/main/cmd/set-config")).toBe(true);
    await watch!.close();
  });

  it("load() retries 3 times then throws GgError(Config) on no reply", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const svc = new RecordingMessagingService();
    // Force request() to reject quickly (no replyBody set) so all 3 attempts fail.
    const origReq = svc.request.bind(svc);
    let attempts = 0;
    svc.request = ((topic, msg) => {
      attempts++;
      return origReq(topic, msg, 5);
    }) as typeof svc.request;
    const src = new ConfigComponentSource(svc, "T", "C");
    await expect(src.load()).rejects.toBeInstanceOf(GgError);
    expect(attempts).toBe(3);
    expect(warn).toHaveBeenCalled();
  });

  it("watch() subscribes to the component's set-config inbox and forwards bodies", async () => {
    const svc = new RecordingMessagingService();
    const src = new ConfigComponentSource(svc, "thing-A", "com.example.C");
    const updates: unknown[] = [];
    const watch = await src.watch((doc) => updates.push(doc));
    const setConfigTopic = "ecv1/thing-A/C/main/cmd/set-config";
    expect(svc.subscriptions.has(setConfigTopic)).toBe(true);

    svc.emit(setConfigTopic, { reloaded: true });
    await tick(0);
    expect(updates).toEqual([{ reloaded: true }]);

    await watch!.close();
    expect(svc.unsubscribed).toContain(setConfigTopic);
  });
});

describe("GreengrassConfigSource", () => {
  it("load() returns getConfiguration for the single key", async () => {
    const client = new FakeIpcClient();
    client.configValue = { greengrass: "value" };
    const src = new GreengrassConfigSource(ipcWith(client), undefined, "ComponentConfig");
    expect(src.sourceName()).toBe("GG_CONFIG");
    expect(await src.load()).toEqual({ greengrass: "value" });
  });

  it("watch() re-fetches and calls onUpdate on a config-update event", async () => {
    const client = new FakeIpcClient();
    client.configValue = { v: 1 };
    const src = new GreengrassConfigSource(ipcWith(client), "other-comp", "ComponentConfig");
    const updates: unknown[] = [];
    const watch = await src.watch((doc) => updates.push(doc));
    expect(client.configStreams).toHaveLength(1);

    client.configValue = { v: 2 };
    client.configStreams[0].fire("message");
    await tick(0);
    expect(updates).toEqual([{ v: 2 }]);
    await watch!.close();
  });
});

describe("ShadowConfigSource", () => {
  const cfgStr = JSON.stringify({ logging: { level: "DEBUG" }, component: { global: { n: 7 } } });

  it("load() extracts desired.ComponentConfig, parses it, and reports it back verbatim", async () => {
    const client = new FakeIpcClient();
    client.shadowBytes = Buffer.from(JSON.stringify({ state: { desired: { ComponentConfig: cfgStr } } }), "utf8");
    const src = new ShadowConfigSource(ipcWith(client), undefined, "thing-A", "com.example.C");
    expect(src.sourceName()).toBe("SHADOW");

    const loaded = await src.load();
    expect(loaded).toEqual(JSON.parse(cfgStr));

    // Reported back verbatim under state.reported.ComponentConfig.
    expect(client.shadowUpdates).toHaveLength(1);
    const reported = JSON.parse(client.shadowUpdates[0].payload.toString("utf8"));
    expect(reported.state.reported.ComponentConfig).toBe(cfgStr);
    // Shadow name defaults to the component name, sanitized to AWS IoT's allowed
    // set (dots -> underscores).
    expect(client.shadowUpdates[0].shadowName).toBe("com_example_C");
  });

  it("load() falls back to a default config when the shadow is missing", async () => {
    const client = new FakeIpcClient(); // shadowBytes undefined -> getThingShadow rejects
    const src = new ShadowConfigSource(ipcWith(client), "myshadow", "thing-A", "C");
    const loaded = (await src.load()) as Record<string, unknown>;
    expect(loaded).toHaveProperty("component");
    expect(loaded).toHaveProperty("logging");
    expect(client.shadowUpdates).toHaveLength(1);
    expect(client.shadowUpdates[0].shadowName).toBe("myshadow");
  });

  it("load() throws GgError(Json) on a non-empty, unparseable shadow", async () => {
    const client = new FakeIpcClient();
    client.shadowBytes = Buffer.from("{ not json", "utf8");
    const src = new ShadowConfigSource(ipcWith(client), undefined, "T", "C");
    await expect(src.load()).rejects.toBeInstanceOf(GgError);
    await src.load().catch((e) => expect((e as GgError).kind).toBe("Json"));
  });

  it("watch() applies a delta: reports it back and forwards the parsed config", async () => {
    const client = new FakeIpcClient();
    const src = new ShadowConfigSource(ipcWith(client), undefined, "thing-A", "C");
    const updates: unknown[] = [];
    const watch = await src.watch((doc) => updates.push(doc));
    expect(client.topicStreams).toHaveLength(1);

    const deltaCfg = JSON.stringify({ component: { global: { changed: true } } });
    const deltaTopic = "$aws/things/thing-A/shadow/name/C/update/delta";
    client.topicStreams[0].fire("message", {
      binaryMessage: {
        context: { topic: deltaTopic },
        message: Buffer.from(JSON.stringify({ state: { ComponentConfig: deltaCfg } }), "utf8"),
      },
    });
    await tick(10);
    expect(updates).toEqual([JSON.parse(deltaCfg)]);
    expect(client.shadowUpdates.length).toBeGreaterThanOrEqual(1);
    const last = client.shadowUpdates[client.shadowUpdates.length - 1];
    expect(JSON.parse(last.payload.toString("utf8")).state.reported.ComponentConfig).toBe(deltaCfg);
    await watch!.close();
  });

  it("load() falls back to state.reported.ComponentConfig when desired is absent", async () => {
    const client = new FakeIpcClient();
    const reported = JSON.stringify({ component: { global: { x: 3 } } });
    client.shadowBytes = Buffer.from(JSON.stringify({ state: { reported: { ComponentConfig: reported } } }), "utf8");
    const src = new ShadowConfigSource(ipcWith(client), undefined, "T", "C");
    expect(await src.load()).toEqual(JSON.parse(reported));
  });

  it("load() uses a default config when the shadow has no ComponentConfig", async () => {
    const client = new FakeIpcClient();
    client.shadowBytes = Buffer.from(JSON.stringify({ state: { desired: {} } }), "utf8");
    const src = new ShadowConfigSource(ipcWith(client), undefined, "T", "C");
    const loaded = (await src.load()) as Record<string, unknown>;
    expect(loaded).toHaveProperty("component");
  });

  it("load() treats an empty shadow as missing (default config)", async () => {
    const client = new FakeIpcClient();
    client.shadowBytes = Buffer.alloc(0);
    const src = new ShadowConfigSource(ipcWith(client), undefined, "T", "C");
    const loaded = (await src.load()) as Record<string, unknown>;
    expect(loaded).toHaveProperty("component");
  });

  it("reportConfig failure during load is caught (no throw)", async () => {
    const client = new FakeIpcClient();
    client.shadowBytes = Buffer.from(JSON.stringify({ state: { desired: { ComponentConfig: cfgStr } } }), "utf8");
    // Make updateThingShadow reject so the catch path runs.
    client.updateThingShadow = async () => {
      throw new Error("update failed");
    };
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const src = new ShadowConfigSource(ipcWith(client), undefined, "T", "C");
    await expect(src.load()).resolves.toEqual(JSON.parse(cfgStr));
    expect(warn).toHaveBeenCalled();
  });

  it("watch() ignores update/accepted and get/accepted events", async () => {
    const client = new FakeIpcClient();
    const src = new ShadowConfigSource(ipcWith(client), undefined, "thing-A", "C");
    const updates: unknown[] = [];
    await src.watch((doc) => updates.push(doc));
    client.topicStreams[0].fire("message", {
      binaryMessage: {
        context: { topic: "$aws/things/thing-A/shadow/name/C/update/accepted" },
        message: Buffer.from("{}", "utf8"),
      },
    });
    await tick(10);
    expect(updates).toHaveLength(0);
    expect(client.shadowUpdates).toHaveLength(0);
  });

  it("watch() reports a default config on get/rejected", async () => {
    const client = new FakeIpcClient();
    const src = new ShadowConfigSource(ipcWith(client), undefined, "thing-A", "C");
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    await src.watch(() => undefined);
    const topic = "$aws/things/thing-A/shadow/name/C/get/rejected";
    client.topicStreams[0].fire("message", {
      binaryMessage: { context: { topic }, message: Buffer.from("{}", "utf8") },
    });
    await tick(10);
    expect(client.shadowUpdates).toHaveLength(1);
    expect(warn).toHaveBeenCalled();
  });
});

describe("buildConfigSource dispatch", () => {
  const opts = { thingName: "T", componentName: "C" };
  it("dispatches FILE and ENV without extra deps", () => {
    expect(buildConfigSource({ kind: "FILE", path: "x.json" }, opts)).toBeInstanceOf(FileConfigSource);
    expect(buildConfigSource({ kind: "ENV", var: "CONFIG" }, opts)).toBeInstanceOf(EnvConfigSource);
  });

  it("CONFIG_COMPONENT requires messaging", () => {
    expect(() => buildConfigSource({ kind: "CONFIG_COMPONENT" }, opts)).toThrow(GgError);
    const svc = new RecordingMessagingService();
    expect(buildConfigSource({ kind: "CONFIG_COMPONENT" }, { ...opts, messaging: svc })).toBeInstanceOf(
      ConfigComponentSource,
    );
  });

  it("GG_CONFIG and SHADOW require the IPC provider", () => {
    expect(() => buildConfigSource({ kind: "GG_CONFIG", key: "ComponentConfig" }, opts)).toThrow(GgError);
    expect(() => buildConfigSource({ kind: "SHADOW" }, opts)).toThrow(GgError);
    const ipc = ipcWith(new FakeIpcClient());
    expect(buildConfigSource({ kind: "GG_CONFIG", key: "K" }, { ...opts, ipcProvider: ipc })).toBeInstanceOf(
      GreengrassConfigSource,
    );
    expect(buildConfigSource({ kind: "SHADOW", name: "s" }, { ...opts, ipcProvider: ipc })).toBeInstanceOf(
      ShadowConfigSource,
    );
  });
});
