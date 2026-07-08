import { describe, it, expect, afterEach } from "vitest";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import { greengrasscoreipc } from "aws-iot-device-sdk-v2";

import { parseArgs } from "../src/cli";
import { Config } from "../src/config/model";
import { validate } from "../src/config/validation";
import {
  buildBaseLayerResolver,
  ensureBaseLayerAllowed,
  LayeredConfigCoordinator,
  parseConfigComponentPayload,
  readComponentControls,
  resolveConfigMapBasePath,
  resolveFileBasePath,
} from "../src/config/layered";
import { ConfigSource, ConfigWatch } from "../src/config/source";
import { deepMerge, JsonObject } from "../src/config/merge";
import { EdgeCommonsError } from "../src/errors";
import { Platform } from "../src/platform";
import { IpcMessagingProvider } from "../src/messaging/ipc-provider";
import { FakeIpcClient, tick } from "./_fakes";

import model = greengrasscoreipc.model;

const savedEnv: Record<string, string | undefined> = {};
const envKeys = ["EDGECOMMONS_SHARED_CONFIG", "EDGECOMMONS_SHARED_COMPONENT"];
const tmp: string[] = [];

function vector<T>(name: string): T {
  return JSON.parse(
    fs.readFileSync(path.resolve(__dirname, "../../../split-config-test-vectors", name), "utf8"),
  ) as T;
}

function tmpFile(contents: string): string {
  const p = path.join(os.tmpdir(), `ec-split-${process.pid}-${Math.random().toString(36).slice(2)}.json`);
  fs.writeFileSync(p, contents);
  tmp.push(p);
  return p;
}

function setEnv(key: string, value: string | undefined): void {
  if (!(key in savedEnv)) savedEnv[key] = process.env[key];
  if (value === undefined) delete process.env[key];
  else process.env[key] = value;
}

function restoreEnv(): void {
  for (const key of envKeys) {
    if (savedEnv[key] === undefined) delete process.env[key];
    else process.env[key] = savedEnv[key];
  }
  for (const key of Object.keys(savedEnv)) delete savedEnv[key];
}

function ipcWith(client: FakeIpcClient): IpcMessagingProvider {
  return IpcMessagingProvider._withClient(
    client as unknown as greengrasscoreipc.Client,
    model.ReceiveMode.RECEIVE_MESSAGES_FROM_OTHERS,
  );
}

afterEach(() => {
  restoreEnv();
  for (const f of tmp.splice(0)) {
    try {
      fs.rmSync(f, { force: true });
    } catch {
      /* ignore */
    }
  }
});

describe("split-config CLI", () => {
  it("parses --no-shared-config as a parse-time flag", () => {
    const parsed = parseArgs(["--platform", "HOST", "--no-shared-config"], {});
    expect(parsed.platform).toBe(Platform.HOST);
    expect(parsed.noSharedConfig).toBe(true);
  });
});

describe("split-config merge vectors", () => {
  const doc = vector<{ cases: Array<{ name: string; input: Record<string, unknown>; expected: Record<string, unknown> }> }>(
    "merge.json",
  );

  for (const c of doc.cases) {
    it(c.name, () => {
      const base = c.input.base as JsonObject | undefined;
      const component = c.input.component as JsonObject;
      if (c.expected.error === "N_LAYER_INHERITANCE_NOT_IMPLEMENTED") {
        expect(() => ensureBaseLayerAllowed(base!)).toThrow(/N_LAYER_INHERITANCE_NOT_IMPLEMENTED/);
        return;
      }
      const options = (c.input.options ?? {}) as Record<string, unknown>;
      const skipBase = options.noSharedConfig === true || component.sharedConfig === false;
      const result = deepMerge(base && !skipBase ? [base, component] : [component]);
      expect(result.effective).toEqual(c.expected.effective);
      if (c.expected.warnings) {
        expect(result.warnings).toEqual(c.expected.warnings);
      }
      if (c.name === "inherited-streaming-streams") {
        const cfg = Config.fromValue("com.example.C", "gw-01", result.effective);
        expect(cfg.raw.streaming).toEqual((c.expected.effective as JsonObject).streaming);
      }
    });
  }
});

describe("split-config resolution vectors", () => {
  const doc = vector<{ cases: Array<{ name: string; provider: string; input: JsonObject; expected: JsonObject }> }>(
    "resolution.json",
  );

  for (const c of doc.cases) {
    it(c.name, async () => {
      switch (c.name) {
        case "file-extends-relative": {
          const actual = resolveFileBasePath(
            c.input.componentPath as string,
            c.input.componentLayer as JsonObject,
          );
          expect(actual.path.replace(/\\/g, "/")).toBe(c.expected.basePath);
          expect(actual.missingIsNoop).toBe(false);
          break;
        }
        case "file-env-var-path": {
          setEnv("EDGECOMMONS_SHARED_CONFIG", ((c.input.env as JsonObject).EDGECOMMONS_SHARED_CONFIG as string));
          const actual = resolveFileBasePath(
            c.input.componentPath as string,
            c.input.componentLayer as JsonObject,
          );
          expect(actual.path).toBe(c.expected.basePath);
          expect(actual.missingIsNoop).toBe(false);
          break;
        }
        case "file-conventional-missing-noop": {
          setEnv("EDGECOMMONS_SHARED_CONFIG", undefined);
          const actual = resolveFileBasePath(
            c.input.componentPath as string,
            c.input.componentLayer as JsonObject,
          );
          expect(actual.path.replace(/\\/g, "/")).toBe(c.expected.basePath);
          expect(actual.missingIsNoop).toBe(true);
          break;
        }
        case "configmap-extends-relative":
        case "configmap-mounted-shared-default": {
          const actual = resolveConfigMapBasePath(
            (c.input.mountDir as string) ?? "/etc/edgecommons",
            c.input.componentPath as string,
            (c.input.componentLayer ?? {}) as JsonObject,
          );
          expect(actual.path.replace(/\\/g, "/")).toBe(c.expected.basePath);
          break;
        }
        case "env-inline-json": {
          setEnv("EDGECOMMONS_SHARED_CONFIG", (c.input.env as JsonObject).EDGECOMMONS_SHARED_CONFIG as string);
          const resolver = buildBaseLayerResolver({ kind: "ENV", var: "CONFIG" }, { thingName: "T", componentName: "C" })!;
          expect(await resolver.resolve({})).toEqual(c.expected.base);
          break;
        }
        case "env-at-path": {
          const p = tmpFile(JSON.stringify({ logging: { level: "INFO" } }));
          setEnv("EDGECOMMONS_SHARED_CONFIG", `@${p}`);
          const resolver = buildBaseLayerResolver({ kind: "ENV", var: "CONFIG" }, { thingName: "T", componentName: "C" })!;
          expect(await resolver.resolve({})).toEqual({ logging: { level: "INFO" } });
          break;
        }
        case "gg-config-default-missing-noop": {
          setEnv("EDGECOMMONS_SHARED_COMPONENT", undefined);
          const client = new FakeIpcClient();
          client.configValue = undefined;
          const resolver = buildBaseLayerResolver(
            { kind: "GG_CONFIG", key: "ComponentConfig" },
            { thingName: "T", componentName: "C", ipcProvider: ipcWith(client) },
          )!;
          expect(await resolver.resolve({})).toBeUndefined();
          break;
        }
        case "gg-config-explicit-env-missing-fails": {
          setEnv("EDGECOMMONS_SHARED_COMPONENT", (c.input.env as JsonObject).EDGECOMMONS_SHARED_COMPONENT as string);
          const client = new FakeIpcClient();
          client.configValue = undefined;
          const resolver = buildBaseLayerResolver(
            { kind: "GG_CONFIG", key: "ComponentConfig" },
            { thingName: "T", componentName: "C", ipcProvider: ipcWith(client) },
          )!;
          await expect(resolver.resolve({})).rejects.toThrow(/SHARED_CONFIG_UNAVAILABLE/);
          break;
        }
        case "shadow-missing-noop": {
          const client = new FakeIpcClient();
          const resolver = buildBaseLayerResolver(
            { kind: "SHADOW" },
            { thingName: "T", componentName: "C", ipcProvider: ipcWith(client) },
          )!;
          expect(await resolver.resolve({})).toBeUndefined();
          break;
        }
        default:
          throw new Error(`unhandled vector ${c.name}`);
      }
    });
  }
});

describe("split-config resolver edge cases", () => {
  it("rejects invalid raw control field types", () => {
    expect(() => readComponentControls({ sharedConfig: "yes" })).toThrow(/sharedConfig/);
    expect(() => readComponentControls({ extends: "" })).toThrow(/extends/);
  });

  it("rejects malformed ENV shared config", async () => {
    setEnv("EDGECOMMONS_SHARED_CONFIG", "{ not json");
    const resolver = buildBaseLayerResolver({ kind: "ENV", var: "CONFIG" }, { thingName: "T", componentName: "C" })!;
    await expect(resolver.resolve({})).rejects.toBeInstanceOf(EdgeCommonsError);
  });

  it("rejects malformed default GG_CONFIG shared config when the key is present", async () => {
    setEnv("EDGECOMMONS_SHARED_COMPONENT", undefined);
    const client = new FakeIpcClient();
    client.configValue = "not-an-object";
    const resolver = buildBaseLayerResolver(
      { kind: "GG_CONFIG", key: "ComponentConfig" },
      { thingName: "T", componentName: "C", ipcProvider: ipcWith(client) },
    )!;

    await expect(resolver.resolve({})).rejects.toBeInstanceOf(EdgeCommonsError);
  });

  it("watches CONFIGMAP shared.json from the mount directory", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ec-split-cm-"));
    tmp.push(path.join(dir, "shared.json"));
    fs.writeFileSync(path.join(dir, "config.json"), JSON.stringify({ component: { token: "c" } }));
    fs.writeFileSync(path.join(dir, "shared.json"), JSON.stringify({ logging: { level: "INFO" } }));
    const resolver = buildBaseLayerResolver(
      { kind: "CONFIGMAP", mountDir: dir, key: "config.json" },
      { thingName: "T", componentName: "C" },
    )!;
    const updates: Array<JsonObject | undefined> = [];
    const watch = await resolver.watch({}, (base) => updates.push(base));

    fs.writeFileSync(path.join(dir, "shared.json"), JSON.stringify({ logging: { level: "WARN" } }));
    for (let i = 0; i < 30 && updates.length === 0; i++) await tick(20);
    expect(updates[updates.length - 1]).toEqual({ logging: { level: "WARN" } });
    await watch?.close();
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it("watches FILE shared config changes", async () => {
    const basePath = tmpFile(JSON.stringify({ logging: { level: "INFO" } }));
    const resolver = buildBaseLayerResolver(
      { kind: "FILE", path: tmpFile(JSON.stringify({ extends: basePath, component: { token: "c" } })) },
      { thingName: "T", componentName: "C" },
    )!;
    const updates: Array<JsonObject | undefined> = [];
    const watch = await resolver.watch({ extends: basePath }, (base) => updates.push(base));
    expect(watch).toBeDefined();

    fs.writeFileSync(basePath, JSON.stringify({ logging: { level: "DEBUG" } }));
    for (let i = 0; i < 30 && updates.length === 0; i++) await tick(20);
    expect(updates[updates.length - 1]).toEqual({ logging: { level: "DEBUG" } });
    await watch?.close();
  });

  it("watches GG_CONFIG shared config updates", async () => {
    const client = new FakeIpcClient();
    client.configValue = { logging: { level: "INFO" } };
    const resolver = buildBaseLayerResolver(
      { kind: "GG_CONFIG", key: "ComponentConfig" },
      { thingName: "T", componentName: "C", ipcProvider: ipcWith(client) },
    )!;
    const updates: Array<JsonObject | undefined> = [];
    const watch = await resolver.watch({}, (base) => updates.push(base));
    expect(client.configStreams).toHaveLength(1);

    client.configValue = { logging: { level: "WARN" } };
    client.configStreams[0].fire("message");
    for (let i = 0; i < 30 && updates.length === 0; i++) await tick(20);
    expect(updates[0]).toEqual({ logging: { level: "WARN" } });
    await watch?.close();
  });

  it("watches SHADOW shared config deltas", async () => {
    const client = new FakeIpcClient();
    const resolver = buildBaseLayerResolver(
      { kind: "SHADOW" },
      { thingName: "T", componentName: "C", ipcProvider: ipcWith(client) },
    )!;
    const updates: Array<JsonObject | undefined> = [];
    const watch = await resolver.watch({}, (base) => updates.push(base));
    expect(client.topicStreams).toHaveLength(1);

    client.topicStreams[0].fire("message", {
      binaryMessage: {
        context: { topic: "$aws/things/T/shadow/name/edgecommons-shared/update/delta" },
        message: Buffer.from(
          JSON.stringify({ state: { ComponentConfig: JSON.stringify({ logging: { level: "WARN" } }) } }),
          "utf8",
        ),
      },
    });
    await tick(0);
    expect(updates[0]).toEqual({ logging: { level: "WARN" } });
    await watch?.close();
  });

  it("rejects malformed existing SHADOW shared ComponentConfig", async () => {
    const client = new FakeIpcClient();
    client.shadowBytes = Buffer.from(
      JSON.stringify({ state: { desired: { ComponentConfig: "{ not json" } } }),
      "utf8",
    );
    const resolver = buildBaseLayerResolver(
      { kind: "SHADOW" },
      { thingName: "T", componentName: "C", ipcProvider: ipcWith(client) },
    )!;
    await expect(resolver.resolve({})).rejects.toBeInstanceOf(EdgeCommonsError);
  });
});

describe("CONFIG_COMPONENT bundle vectors", () => {
  const doc = vector<{ cases: Array<{ name: string; input: JsonObject; expected: JsonObject }> }>(
    "config-component-bundles.json",
  );

  for (const c of doc.cases) {
    it(c.name, () => {
      const body = c.input.body ?? c.input.push;
      if (c.expected.error) {
        expect(() => parseConfigComponentPayload(body)).toThrow(String(c.expected.error));
        return;
      }
      const parsed = parseConfigComponentPayload(body);
      if (c.expected.base === null) {
        expect(parsed.baseLayer).toBeUndefined();
      }
      if (c.expected.component) {
        expect(parsed.componentLayer).toEqual(c.expected.component);
      }
      if (c.expected.effective) {
        const result = deepMerge(parsed.baseLayer ? [parsed.baseLayer, parsed.componentLayer] : [parsed.componentLayer]);
        expect(result.effective).toEqual(c.expected.effective);
      }
    });
  }
});

class MutableSource implements ConfigSource {
  update?: (raw: unknown) => void;

  constructor(public raw: unknown) {}

  async load(): Promise<unknown> {
    return this.raw;
  }

  sourceName(): string {
    return "FILE";
  }

  async watch(onUpdate: (raw: unknown) => void): Promise<ConfigWatch> {
    this.update = onUpdate;
    return { close: async () => undefined };
  }
}

describe("LayeredConfigCoordinator", () => {
  it("loads FILE shared config, strips controls, and rejects invalid reloads while keeping current", async () => {
    const basePath = tmpFile(JSON.stringify({ logging: { level: "INFO" }, tags: { inherited: "yes" } }));
    const source = new MutableSource({
      extends: basePath,
      component: { token: "opcua-adapter" },
      tags: { component: "one" },
    });
    const resolver = buildBaseLayerResolver(
      { kind: "FILE", path: tmpFile(JSON.stringify(source.raw)) },
      { thingName: "gw-01", componentName: "com.example.C" },
    );
    const coordinator = new LayeredConfigCoordinator({
      source,
      sourceSpec: { kind: "FILE", path: "config.json" },
      baseResolver: resolver,
      noSharedConfig: false,
    });

    const initial = await coordinator.loadEffective();
    validate(initial);
    expect(initial).toEqual({
      logging: { level: "INFO" },
      tags: { inherited: "yes", component: "one" },
      component: { token: "opcua-adapter" },
    });
    expect(initial).not.toHaveProperty("extends");

    const accepted: JsonObject[] = [];
    const watch = await coordinator.watch((effective) => {
      try {
        validate(effective);
        accepted.push(effective);
        return true;
      } catch {
        return false;
      }
    });

    source.update?.({
      extends: basePath,
      metricEmission: { target: "not-a-target" },
      component: { token: "opcua-adapter" },
      tags: { component: "bad" },
    });
    for (let i = 0; i < 20 && accepted.length === 0; i++) await tick(10);
    expect(accepted).toHaveLength(0);

    source.update?.({
      extends: basePath,
      logging: { level: "DEBUG" },
      component: { token: "opcua-adapter" },
      tags: { component: "two" },
    });
    for (let i = 0; i < 20 && accepted.length === 0; i++) await tick(10);
    expect(accepted).toHaveLength(1);
    expect(accepted[0].logging).toEqual({ level: "DEBUG" });
    expect(accepted[0].tags).toEqual({ inherited: "yes", component: "two" });
    await watch?.close();
  });

  it("honors --no-shared-config over a component sharedConfig true control", async () => {
    const basePath = tmpFile(JSON.stringify({ logging: { level: "INFO" } }));
    const source = new MutableSource({
      extends: basePath,
      sharedConfig: true,
      component: { token: "opcua-adapter" },
    });
    const coordinator = new LayeredConfigCoordinator({
      source,
      sourceSpec: { kind: "FILE", path: "config.json" },
      baseResolver: buildBaseLayerResolver(
        { kind: "FILE", path: "config.json" },
        { thingName: "gw-01", componentName: "com.example.C" },
      ),
      noSharedConfig: true,
    });

    expect(await coordinator.loadEffective()).toEqual({ component: { token: "opcua-adapter" } });
  });

  it("stores CONFIG_COMPONENT bundle replies as merged effective config only", async () => {
    const source = new MutableSource({
      base: { logging: { level: "WARN" }, extends: "site.json" },
      component: { component: { token: "opcua-adapter" }, sharedConfig: true },
    });
    const coordinator = new LayeredConfigCoordinator({
      source,
      sourceSpec: { kind: "CONFIG_COMPONENT" },
      noSharedConfig: false,
    });

    await expect(coordinator.loadEffective()).rejects.toBeInstanceOf(EdgeCommonsError);

    source.raw = {
      base: { logging: { level: "WARN" } },
      component: { component: { token: "opcua-adapter" }, sharedConfig: true },
    };
    const effective = await coordinator.loadEffective();
    expect(effective).toEqual({ logging: { level: "WARN" }, component: { token: "opcua-adapter" } });
    expect(effective).not.toHaveProperty("sharedConfig");
  });

  it("preserves the previous CONFIG_COMPONENT base for legacy set-config pushes only", async () => {
    const source = new MutableSource({
      base: { logging: { level: "INFO" }, tags: { site: "dallas" } },
      component: { component: { global: { v: 1 } } },
    });
    const coordinator = new LayeredConfigCoordinator({
      source,
      sourceSpec: { kind: "CONFIG_COMPONENT" },
      noSharedConfig: false,
    });

    expect(await coordinator.loadEffective()).toEqual({
      logging: { level: "INFO" },
      tags: { site: "dallas" },
      component: { global: { v: 1 } },
    });

    const accepted: JsonObject[] = [];
    const watch = await coordinator.watch((effective) => {
      accepted.push(effective);
      return true;
    });

    source.update?.({ component: { global: { v: 2 } }, tags: { component: "split" } });
    for (let i = 0; i < 20 && accepted.length === 0; i++) await tick(10);
    expect(accepted[0]).toEqual({
      logging: { level: "INFO" },
      tags: { site: "dallas", component: "split" },
      component: { global: { v: 2 } },
    });

    source.update?.({ base: null, component: { component: { global: { v: 3 } } } });
    for (let i = 0; i < 20 && accepted.length < 2; i++) await tick(10);
    expect(accepted[1]).toEqual({ component: { global: { v: 3 } } });
    await watch?.close();
  });
});
