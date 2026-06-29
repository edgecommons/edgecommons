import { describe, it, expect } from "vitest";
import { join } from "path";

import { parseArgs } from "../src/cli";
import { Platform, Transport } from "../src/platform";
import { GgError } from "../src/errors";

// Empty env so auto-detection is deterministic (no Greengrass/Kubernetes signals -> HOST).
const NO_ENV = {} as Record<string, string | undefined>;

describe("parseArgs", () => {
  it("with no platform flag auto-detects (HOST here -> MQTT) and defaults config to FILE", () => {
    const parsed = parseArgs([], NO_ENV);
    expect(parsed.platform).toBe(Platform.HOST);
    expect(parsed.transport).toBe(Transport.MQTT);
    expect(parsed.config).toEqual({ kind: "FILE", path: "config.json" });
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

  it("KUBERNETES platform resolves to MQTT + CONFIGMAP (Phase 1a)", () => {
    const parsed = parseArgs(["--platform", "KUBERNETES"], NO_ENV);
    expect(parsed.platform).toBe(Platform.KUBERNETES);
    expect(parsed.transport).toBe(Transport.MQTT);
    expect(parsed.config).toEqual({ kind: "CONFIGMAP", mountDir: undefined, key: undefined });
  });

  it("KUBERNETES with IPC transport fails the IPC lock", () => {
    try {
      parseArgs(["--platform", "KUBERNETES", "--transport", "IPC"], NO_ENV);
      throw new Error("expected throw");
    } catch (e) {
      expect(e).toBeInstanceOf(GgError);
      expect((e as GgError).kind).toBe("Cli");
      expect((e as GgError).message).toContain("IPC transport requires --platform GREENGRASS");
    }
  });

  it("CONFIGMAP without args (defaults applied in the source)", () => {
    const parsed = parseArgs(["--platform", "KUBERNETES", "-c", "CONFIGMAP"], NO_ENV);
    expect(parsed.config).toEqual({ kind: "CONFIGMAP", mountDir: undefined, key: undefined });
  });

  it("CONFIGMAP with explicit mount dir and key", () => {
    const parsed = parseArgs(
      ["--platform", "KUBERNETES", "-c", "CONFIGMAP", "/etc/myconf", "app.json"],
      NO_ENV,
    );
    expect(parsed.config).toEqual({ kind: "CONFIGMAP", mountDir: "/etc/myconf", key: "app.json" });
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

  // ---------- FR-MSG-1: default messaging-config path from CONFIGMAP ----------

  it("KUBERNETES (CONFIGMAP + MQTT) defaults messagingConfigPath to /etc/ggcommons/config.json", () => {
    // No explicit `--transport MQTT <path>` is needed: the single mounted ConfigMap file doubles as
    // both the messaging-config and the component config.
    const parsed = parseArgs(["--platform", "KUBERNETES"], NO_ENV);
    expect(parsed.transport).toBe(Transport.MQTT);
    expect(parsed.config).toEqual({ kind: "CONFIGMAP", mountDir: undefined, key: undefined });
    expect(parsed.messagingConfigPath).toBe(join("/etc/ggcommons", "config.json"));
  });

  it("CONFIGMAP + MQTT default uses the explicit -c CONFIGMAP mount dir + key", () => {
    const parsed = parseArgs(
      ["--platform", "KUBERNETES", "-c", "CONFIGMAP", "/etc/myconf", "app.json"],
      NO_ENV,
    );
    expect(parsed.messagingConfigPath).toBe(join("/etc/myconf", "app.json"));
  });

  it("CONFIGMAP + MQTT default uses the explicit mount dir but the default key when key omitted", () => {
    const parsed = parseArgs(["--platform", "KUBERNETES", "-c", "CONFIGMAP", "/etc/myconf"], NO_ENV);
    expect(parsed.messagingConfigPath).toBe(join("/etc/myconf", "config.json"));
  });

  it("an explicit --transport MQTT <path> still wins over the CONFIGMAP default", () => {
    const parsed = parseArgs(
      ["--platform", "KUBERNETES", "--transport", "MQTT", "/custom/messaging.json"],
      NO_ENV,
    );
    expect(parsed.config.kind).toBe("CONFIGMAP");
    expect(parsed.messagingConfigPath).toBe("/custom/messaging.json");
  });

  it("HOST + MQTT does NOT get a default messaging path (HOST defaults to FILE, not CONFIGMAP)", () => {
    // Only CONFIGMAP+MQTT synthesizes a messaging path: HOST defaults to FILE, so MQTT still requires
    // an explicit messaging-config path (enforced later at provider build); parseArgs leaves it undefined.
    const parsed = parseArgs(["--platform", "HOST"], NO_ENV);
    expect(parsed.transport).toBe(Transport.MQTT);
    expect(parsed.config).toEqual({ kind: "FILE", path: "config.json" });
    expect(parsed.messagingConfigPath).toBeUndefined();
  });

  it("HOST + MQTT + explicit -c CONFIGMAP also gets the default path (CONFIGMAP+MQTT is the trigger, not the platform)", () => {
    const parsed = parseArgs(["--platform", "HOST", "-c", "CONFIGMAP"], NO_ENV);
    expect(parsed.config.kind).toBe("CONFIGMAP");
    expect(parsed.messagingConfigPath).toBe(join("/etc/ggcommons", "config.json"));
  });

  it("CONFIGMAP with a non-MQTT path does not apply the messaging default (no MQTT, no messaging)", () => {
    // GREENGRASS + IPC + explicit -c CONFIGMAP: IPC transport means no MQTT messaging-config default.
    const parsed = parseArgs(["--platform", "GREENGRASS", "-c", "CONFIGMAP"], NO_ENV);
    expect(parsed.transport).toBe(Transport.IPC);
    expect(parsed.config.kind).toBe("CONFIGMAP");
    expect(parsed.messagingConfigPath).toBeUndefined();
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
