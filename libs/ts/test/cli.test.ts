import { describe, it, expect } from "vitest";

import { parseArgs } from "../src/cli";
import { GgError } from "../src/errors";

describe("parseArgs", () => {
  it("defaults to GREENGRASS mode and GG_CONFIG source with key ComponentConfig", () => {
    const parsed = parseArgs([]);
    expect(parsed.mode).toEqual({ kind: "GREENGRASS" });
    expect(parsed.config).toEqual({ kind: "GG_CONFIG", key: "ComponentConfig" });
    expect(parsed.thing).toBeUndefined();
  });

  it("FILE with explicit path", () => {
    const parsed = parseArgs(["-c", "FILE", "/etc/config.json"]);
    expect(parsed.config).toEqual({ kind: "FILE", path: "/etc/config.json" });
  });

  it("FILE without a path defaults to config.json", () => {
    const parsed = parseArgs(["-c", "FILE"]);
    expect(parsed.config).toEqual({ kind: "FILE", path: "config.json" });
  });

  it("ENV with explicit variable name", () => {
    const parsed = parseArgs(["-c", "ENV", "MY_CONFIG"]);
    expect(parsed.config).toEqual({ kind: "ENV", var: "MY_CONFIG" });
  });

  it("ENV without a name defaults to CONFIG", () => {
    const parsed = parseArgs(["-c", "ENV"]);
    expect(parsed.config).toEqual({ kind: "ENV", var: "CONFIG" });
  });

  it("GG_CONFIG with component and key", () => {
    const parsed = parseArgs(["-c", "GG_CONFIG", "com.example.Other", "MyKey"]);
    expect(parsed.config).toEqual({
      kind: "GG_CONFIG",
      component: "com.example.Other",
      key: "MyKey",
    });
  });

  it("GG_CONFIG defaults the key", () => {
    const parsed = parseArgs(["-c", "GG_CONFIG", "com.example.Other"]);
    expect(parsed.config).toEqual({
      kind: "GG_CONFIG",
      component: "com.example.Other",
      key: "ComponentConfig",
    });
  });

  it("SHADOW with a name", () => {
    const parsed = parseArgs(["-c", "SHADOW", "myShadow"]);
    expect(parsed.config).toEqual({ kind: "SHADOW", name: "myShadow" });
  });

  it("CONFIG_COMPONENT", () => {
    const parsed = parseArgs(["-c", "CONFIG_COMPONENT"]);
    expect(parsed.config).toEqual({ kind: "CONFIG_COMPONENT" });
  });

  it("config source is case-insensitive", () => {
    const parsed = parseArgs(["-c", "file", "/x.json"]);
    expect(parsed.config).toEqual({ kind: "FILE", path: "/x.json" });
  });

  it("STANDALONE requires a path", () => {
    expect(() => parseArgs(["-m", "STANDALONE"])).toThrow(GgError);
    try {
      parseArgs(["-m", "STANDALONE"]);
    } catch (e) {
      expect((e as GgError).kind).toBe("Cli");
    }
  });

  it("STANDALONE with a path parses", () => {
    const parsed = parseArgs(["-m", "STANDALONE", "messaging.json"]);
    expect(parsed.mode).toEqual({
      kind: "STANDALONE",
      messagingConfigPath: "messaging.json",
    });
  });

  it("GREENGRASS mode explicit", () => {
    const parsed = parseArgs(["-m", "GREENGRASS"]);
    expect(parsed.mode).toEqual({ kind: "GREENGRASS" });
  });

  it("-t takes the full value, never truncated", () => {
    const parsed = parseArgs(["-t", "my-long-thing-name-123"]);
    expect(parsed.thing).toBe("my-long-thing-name-123");
  });

  it("--thing long form also works", () => {
    const parsed = parseArgs(["--thing", "abc"]);
    expect(parsed.thing).toBe("abc");
  });

  it("-t without a value throws", () => {
    expect(() => parseArgs(["-t"])).toThrow(GgError);
  });

  it("unknown config source throws Cli", () => {
    try {
      parseArgs(["-c", "NOPE"]);
      throw new Error("expected throw");
    } catch (e) {
      expect(e).toBeInstanceOf(GgError);
      expect((e as GgError).kind).toBe("Cli");
    }
  });

  it("unknown mode throws Cli", () => {
    try {
      parseArgs(["-m", "BOGUS"]);
      throw new Error("expected throw");
    } catch (e) {
      expect(e).toBeInstanceOf(GgError);
      expect((e as GgError).kind).toBe("Cli");
    }
  });

  it("unexpected argument throws Cli", () => {
    expect(() => parseArgs(["--frobnicate"])).toThrow(GgError);
  });

  it("combines mode, config and thing", () => {
    const parsed = parseArgs([
      "-m",
      "STANDALONE",
      "m.json",
      "-c",
      "FILE",
      "c.json",
      "-t",
      "thing-9",
    ]);
    expect(parsed.mode).toEqual({ kind: "STANDALONE", messagingConfigPath: "m.json" });
    expect(parsed.config).toEqual({ kind: "FILE", path: "c.json" });
    expect(parsed.thing).toBe("thing-9");
  });
});
