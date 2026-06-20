import { describe, it, expect } from "vitest";

import { Config } from "../src/config/model";
import { resolve } from "../src/config/template";

describe("template resolve", () => {
  it("substitutes builtins and tags", () => {
    const cfg = Config.fromValue("com.example.MyComponent", "thing-7", {
      tags: { site: "factory-1" },
    });
    expect(resolve(cfg, "heartbeat/{ThingName}/{ComponentName}")).toBe(
      "heartbeat/thing-7/MyComponent",
    );
    expect(resolve(cfg, "/var/log/{site}.log")).toBe("/var/log/factory-1.log");
  });

  it("{ComponentName} is short, {ComponentFullName} is full", () => {
    const cfg = Config.fromValue("com.example.MyComponent", "t", {});
    expect(resolve(cfg, "{ComponentName}")).toBe("MyComponent");
    expect(resolve(cfg, "{ComponentFullName}")).toBe("com.example.MyComponent");

    const cfg2 = Config.fromValue("Simple", "t", {});
    expect(resolve(cfg2, "{ComponentName}")).toBe("Simple");
    expect(resolve(cfg2, "{ComponentFullName}")).toBe("Simple");
  });

  it("leaves unknown placeholders untouched", () => {
    const cfg = Config.fromValue("c", "t", {});
    expect(resolve(cfg, "{Unknown}")).toBe("{Unknown}");
  });

  it("sanitizes path traversal and topic wildcards in values, preserving template separators", () => {
    const cfg = Config.fromValue("com.example.C", "../../etc/passwd", {
      tags: { evil: "a/+/#" },
    });
    expect(resolve(cfg, "/logs/{ThingName}.log")).toBe("/logs/____etc_passwd.log");
    expect(resolve(cfg, "t/{evil}/x")).toBe("t/a____/x");
  });

  it("preserves template separators and clean values", () => {
    const cfg = Config.fromValue("com.example.MyComponent", "thing-7", {});
    expect(resolve(cfg, "{ThingName}/{ComponentName}/metric")).toBe(
      "thing-7/MyComponent/metric",
    );
  });
});
