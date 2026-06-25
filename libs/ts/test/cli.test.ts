import { describe, it, expect } from "vitest";

import { parseArgs } from "../src/cli";
import { Platform, Transport } from "../src/platform";
import { GgError } from "../src/errors";

// Empty env so auto-detection is deterministic (no Greengrass/Kubernetes signals -> HOST).
const NO_ENV = {} as Record<string, string | undefined>;

describe("parseArgs", () => {
  it("with no platform flag auto-detects (HOST here -> MQTT) and defaults config to GG_CONFIG", () => {
    const parsed = parseArgs([], NO_ENV);
    expect(parsed.platform).toBe(Platform.HOST);
    expect(parsed.transport).toBe(Transport.MQTT);
    expect(parsed.config).toEqual({ kind: "GG_CONFIG", key: "ComponentConfig" });
    expect(parsed.thing).toBe("NOT_GREENGRASS");
  });

  it("explicit GREENGRASS platform derives IPC transport", () => {
    const parsed = parseArgs(["--platform", "GREENGRASS"], NO_ENV);
    expect(parsed.platform).toBe(Platform.GREENGRASS);
    expect(parsed.transport).toBe(Transport.IPC);
    expect(parsed.messagingConfigPath).toBeUndefined();
  });

  it("FILE with explicit path", () => {
    const parsed = parseArgs(["--platform", "GREENGRASS", "-c", "FILE", "/etc/config.json"], NO_ENV);
    expect(parsed.config).toEqual({ kind: "FILE", path: "/etc/config.json" });
  });

  it("FILE without a path defaults to config.json", () => {
    const parsed = parseArgs(["--platform", "GREENGRASS", "-c", "FILE"], NO_ENV);
    expect(parsed.config).toEqual({ kind: "FILE", path: "config.json" });
  });

  it("ENV with explicit variable name", () => {
    const parsed = parseArgs(["--platform", "GREENGRASS", "-c", "ENV", "MY_CONFIG"], NO_ENV);
    expect(parsed.config).toEqual({ kind: "ENV", var: "MY_CONFIG" });
  });

  it("ENV without a name defaults to CONFIG", () => {
    const parsed = parseArgs(["--platform", "GREENGRASS", "-c", "ENV"], NO_ENV);
    expect(parsed.config).toEqual({ kind: "ENV", var: "CONFIG" });
  });

  it("GG_CONFIG with component and key", () => {
    const parsed = parseArgs(
      ["--platform", "GREENGRASS", "-c", "GG_CONFIG", "com.example.Other", "MyKey"],
      NO_ENV,
    );
    expect(parsed.config).toEqual({
      kind: "GG_CONFIG",
      component: "com.example.Other",
      key: "MyKey",
    });
  });

  it("GG_CONFIG defaults the key", () => {
    const parsed = parseArgs(["--platform", "GREENGRASS", "-c", "GG_CONFIG", "com.example.Other"], NO_ENV);
    expect(parsed.config).toEqual({
      kind: "GG_CONFIG",
      component: "com.example.Other",
      key: "ComponentConfig",
    });
  });

  it("SHADOW with a name", () => {
    const parsed = parseArgs(["--platform", "GREENGRASS", "-c", "SHADOW", "myShadow"], NO_ENV);
    expect(parsed.config).toEqual({ kind: "SHADOW", name: "myShadow" });
  });

  it("CONFIG_COMPONENT", () => {
    const parsed = parseArgs(["--platform", "GREENGRASS", "-c", "CONFIG_COMPONENT"], NO_ENV);
    expect(parsed.config).toEqual({ kind: "CONFIG_COMPONENT" });
  });

  it("config source is case-insensitive", () => {
    const parsed = parseArgs(["--platform", "GREENGRASS", "-c", "file", "/x.json"], NO_ENV);
    expect(parsed.config).toEqual({ kind: "FILE", path: "/x.json" });
  });

  it("MQTT transport with a messaging-config path parses", () => {
    const parsed = parseArgs(["--platform", "HOST", "--transport", "MQTT", "messaging.json"], NO_ENV);
    expect(parsed.platform).toBe(Platform.HOST);
    expect(parsed.transport).toBe(Transport.MQTT);
    expect(parsed.messagingConfigPath).toBe("messaging.json");
  });

  it("MQTT transport without a path parses (path enforced later at provider build)", () => {
    const parsed = parseArgs(["--platform", "HOST", "--transport", "MQTT"], NO_ENV);
    expect(parsed.transport).toBe(Transport.MQTT);
    expect(parsed.messagingConfigPath).toBeUndefined();
  });

  it("platform/transport tokens are case-insensitive", () => {
    const parsed = parseArgs(["--platform", "host", "--transport", "mqtt", "m.json"], NO_ENV);
    expect(parsed.platform).toBe(Platform.HOST);
    expect(parsed.transport).toBe(Transport.MQTT);
    expect(parsed.messagingConfigPath).toBe("m.json");
  });

  it("'auto' platform behaves like omitting --platform (detection runs)", () => {
    const parsed = parseArgs(["--platform", "auto", "--transport", "MQTT", "m.json"], NO_ENV);
    expect(parsed.platform).toBe(Platform.HOST);
    expect(parsed.transport).toBe(Transport.MQTT);
  });

  it("-t takes the full value, never truncated", () => {
    const parsed = parseArgs(["--platform", "GREENGRASS", "-t", "my-long-thing-name-123"], NO_ENV);
    expect(parsed.thing).toBe("my-long-thing-name-123");
  });

  it("--thing long form also works", () => {
    const parsed = parseArgs(["--platform", "GREENGRASS", "--thing", "abc"], NO_ENV);
    expect(parsed.thing).toBe("abc");
  });

  it("-t without a value throws", () => {
    expect(() => parseArgs(["--platform", "GREENGRASS", "-t"], NO_ENV)).toThrow(GgError);
  });

  it("identity falls back to AWS_IOT_THING_NAME env when -t absent", () => {
    const parsed = parseArgs(["--platform", "GREENGRASS"], { AWS_IOT_THING_NAME: "env-thing" });
    expect(parsed.thing).toBe("env-thing");
  });

  it("unknown config source throws Cli", () => {
    try {
      parseArgs(["--platform", "GREENGRASS", "-c", "NOPE"], NO_ENV);
      throw new Error("expected throw");
    } catch (e) {
      expect(e).toBeInstanceOf(GgError);
      expect((e as GgError).kind).toBe("Cli");
    }
  });

  it("unknown platform throws Cli", () => {
    try {
      parseArgs(["--platform", "BOGUS"], NO_ENV);
      throw new Error("expected throw");
    } catch (e) {
      expect(e).toBeInstanceOf(GgError);
      expect((e as GgError).kind).toBe("Cli");
    }
  });

  it("unknown transport throws Cli", () => {
    try {
      parseArgs(["--platform", "HOST", "--transport", "BOGUS"], NO_ENV);
      throw new Error("expected throw");
    } catch (e) {
      expect(e).toBeInstanceOf(GgError);
      expect((e as GgError).kind).toBe("Cli");
    }
  });

  it("IPC on HOST fails the IPC lock", () => {
    try {
      parseArgs(["--platform", "HOST", "--transport", "IPC"], NO_ENV);
      throw new Error("expected throw");
    } catch (e) {
      expect(e).toBeInstanceOf(GgError);
      expect((e as GgError).kind).toBe("Cli");
      expect((e as GgError).message).toContain("IPC transport requires --platform GREENGRASS");
    }
  });

  it("KUBERNETES platform fails fast in Phase 0", () => {
    try {
      parseArgs(["--platform", "KUBERNETES"], NO_ENV);
      throw new Error("expected throw");
    } catch (e) {
      expect(e).toBeInstanceOf(GgError);
      expect((e as GgError).message).toContain("KUBERNETES");
    }
  });

  it("legacy -m flag is rejected with guidance", () => {
    try {
      parseArgs(["-m", "STANDALONE", "m.json"], NO_ENV);
      throw new Error("expected throw");
    } catch (e) {
      expect(e).toBeInstanceOf(GgError);
      expect((e as GgError).kind).toBe("Cli");
      expect((e as GgError).message).toContain("--platform");
      expect((e as GgError).message).toContain("--transport");
    }
  });

  it("legacy --mode flag is rejected with guidance", () => {
    expect(() => parseArgs(["--mode", "GREENGRASS"], NO_ENV)).toThrow(GgError);
  });

  it("unexpected argument throws Cli", () => {
    expect(() => parseArgs(["--frobnicate"], NO_ENV)).toThrow(GgError);
  });

  it("combines platform, transport, config and thing", () => {
    const parsed = parseArgs(
      [
        "--platform",
        "HOST",
        "--transport",
        "MQTT",
        "m.json",
        "-c",
        "FILE",
        "c.json",
        "-t",
        "thing-9",
      ],
      NO_ENV,
    );
    expect(parsed.platform).toBe(Platform.HOST);
    expect(parsed.transport).toBe(Transport.MQTT);
    expect(parsed.messagingConfigPath).toBe("m.json");
    expect(parsed.config).toEqual({ kind: "FILE", path: "c.json" });
    expect(parsed.thing).toBe("thing-9");
  });
});
