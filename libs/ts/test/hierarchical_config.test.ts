import { describe, it, expect } from "vitest";
import * as fs from "fs";
import * as path from "path";

import { parseArgs } from "../src/cli";
import {
  LayeredConfigCoordinator,
  mergeLineageLayers,
  parseConfigComponentPayload,
} from "../src/config/layered";
import { ConfigSource, ConfigWatch } from "../src/config/source";
import { deepMerge, JsonObject } from "../src/config/merge";
import { validate } from "../src/config/validation";

function vector<T>(name: string): T {
  return JSON.parse(
    fs.readFileSync(path.resolve(__dirname, "../../../hierarchical-config-test-vectors", name), "utf8"),
  ) as T;
}

class MutableSource implements ConfigSource {
  update?: (raw: unknown) => void;

  constructor(
    public raw: unknown,
    private readonly name = "CONFIG_COMPONENT",
  ) {}

  async load(): Promise<unknown> {
    return this.raw;
  }

  sourceName(): string {
    return this.name;
  }

  async watch(onUpdate: (raw: unknown) => void): Promise<ConfigWatch> {
    this.update = onUpdate;
    return { close: async () => undefined };
  }
}

async function tick(ms = 0): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, ms));
}

describe("hierarchical config CLI cutover", () => {
  it("rejects the removed --no-shared-config flag", () => {
    expect(() => parseArgs(["--platform", "HOST", "--no-shared-config"], {})).toThrow(/unexpected argument/);
  });
});

describe("hierarchical config merge vectors", () => {
  const doc = vector<{ cases: Array<{ name: string; input: { layers: Array<{ config: JsonObject }> }; expected: JsonObject }> }>(
    "merge.json",
  );

  for (const c of doc.cases) {
    it(c.name, () => {
      const result = deepMerge(c.input.layers.map((layer) => layer.config));
      expect(result.effective).toEqual(c.expected.effective);
      if (c.expected.warnings) {
        expect(result.warnings).toEqual(c.expected.warnings);
      }
    });
  }
});

describe("CONFIG_COMPONENT lineage bundle vectors", () => {
  const doc = vector<{
    cases: Array<{
      name: string;
      input: { requestComponent: string; body: unknown };
      expected: { effective?: JsonObject; error?: string };
    }>;
  }>("lineage-bundles.json");

  for (const c of doc.cases) {
    it(c.name, () => {
      if (c.expected.error) {
        expect(() => parseConfigComponentPayload(c.input.body, c.input.requestComponent)).toThrow(c.expected.error);
        return;
      }
      const bundle = parseConfigComponentPayload(c.input.body, c.input.requestComponent);
      const result = mergeLineageLayers(bundle.layers);
      expect(result.effective).toEqual(c.expected.effective);
      validate(result.effective);
    });
  }
});

describe("LayeredConfigCoordinator", () => {
  it("treats direct providers as a single effective document", async () => {
    const raw = {
      component: { token: "opcua-adapter" },
      unknownTopLevel: { retained: true },
    };
    const coordinator = new LayeredConfigCoordinator({
      source: new MutableSource(raw, "FILE"),
      sourceSpec: { kind: "FILE", path: "config.json" },
      componentName: "opcua-adapter",
    });

    expect(await coordinator.loadEffective()).toEqual(raw);
  });

  it("keeps the previous effective CONFIG_COMPONENT snapshot when a push is invalid", async () => {
    const errors = vector<{
      cases: Array<{
        name: string;
        input: { previousEffective: JsonObject; push?: unknown; body?: unknown };
        expected: {
          effective?: JsonObject;
          error?: string;
          notifyListeners?: boolean;
        };
      }>;
    }>("errors.json");

    const previousEffective = errors.cases[0].input.previousEffective;
    const source = new MutableSource({
      lineageVersion: 1,
      catalogVersion: "previous",
      component: "opcua-adapter",
      layers: [{ id: "component/opcua-adapter", kind: "component", component: "opcua-adapter", config: previousEffective }],
    });
    const coordinator = new LayeredConfigCoordinator({
      source,
      sourceSpec: { kind: "CONFIG_COMPONENT" },
      componentName: "opcua-adapter",
    });
    const initial = await coordinator.loadEffective();
    expect(initial).toEqual(previousEffective);

    const accepted: JsonObject[] = [];
    const watch = await coordinator.watch((effective) => {
      try {
        validate(effective);
      } catch {
        return false;
      }
      accepted.push(effective);
      return true;
    });

    for (const c of errors.cases) {
      if (c.name === "valid-push-replaces-previous-effective") continue;
      source.update?.(c.input.push ?? c.input.body);
      await tick(0);
      expect(accepted).toHaveLength(0);
      expect(coordinator.latestSnapshot()).toEqual(c.expected.effective ?? c.input.previousEffective);
    }

    const valid = errors.cases.find((c) => c.name === "valid-push-replaces-previous-effective")!;
    source.update?.(valid.input.push);
    await tick(0);
    expect(accepted).toEqual([valid.expected.effective]);
    expect(coordinator.latestSnapshot()).toEqual(valid.expected.effective);
    await watch?.close();
  });

  it("serializes asynchronous candidate application so generations commit in source order", async () => {
    const source = new MutableSource({ generation: 0 }, "FILE");
    const coordinator = new LayeredConfigCoordinator({
      source,
      sourceSpec: { kind: "FILE", path: "config.json" },
      componentName: "camera-adapter",
    });
    await coordinator.loadEffective();
    const accepted: number[] = [];
    let beganFirst!: () => void;
    let releaseFirst!: () => void;
    const firstBegan = new Promise<void>((resolve) => { beganFirst = resolve; });
    const firstReleased = new Promise<void>((resolve) => { releaseFirst = resolve; });
    const watch = await coordinator.watch(async (candidate) => {
      const generation = candidate.generation as number;
      if (generation === 1) {
        beganFirst();
        await firstReleased;
      }
      accepted.push(generation);
      return true;
    });

    source.update?.({ generation: 1 });
    await firstBegan;
    source.update?.({ generation: 2 });
    releaseFirst();
    await tick(10);

    expect(accepted).toEqual([1, 2]);
    expect(coordinator.latestSnapshot()).toEqual({ generation: 2 });
    await watch?.close();
  });
});
