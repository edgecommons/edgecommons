import { describe, it, expect } from "vitest";

import { Config } from "../src/config/model";
import { isIsoControl, resolve, sanitize } from "../src/config/template";

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
    const cfg = Config.fromValue("c", "t", {
      hierarchy: { levels: ["site", "device"] },
      identity: { site: "factory-1" },
    });
    expect(resolve(cfg, "{Unknown}/{site}")).toBe("{Unknown}/factory-1");
  });

  it("substitutes hierarchy identity names without tags", () => {
    const cfg = Config.fromValue("com.example.MyComponent", "gw-01", {
      hierarchy: { levels: ["site", "line", "device"] },
      identity: {
        site: "factory/1",
        line: "line+2",
      },
    });

    expect(resolve(cfg, "{site}/{line}/{device}/{ThingName}")).toBe(
      "factory_1/line_2/gw-01/gw-01",
    );
  });

  it("identity placeholders win over colliding tags", () => {
    const cfg = Config.fromValue("com.example.MyComponent", "gw-01", {
      hierarchy: { levels: ["site", "device"] },
      identity: { site: "identity-site" },
      tags: {
        site: "tag-site",
        device: "tag-device",
        zone: "tag-zone",
      },
    });

    expect(resolve(cfg, "{site}/{device}/{zone}")).toBe("identity-site/gw-01/tag-zone");
  });

  it("builtins win over identity and tags with the same symbol", () => {
    const cfg = Config.fromValue("com.example.MyComponent", "gw-01", {
      hierarchy: { levels: ["ThingName", "device"] },
      identity: { ThingName: "identity-thing" },
      tags: {
        ThingName: "tag-thing",
        ComponentName: "tag-component",
        ComponentFullName: "tag-full",
      },
    });

    expect(resolve(cfg, "{ThingName}/{ComponentName}/{ComponentFullName}")).toBe(
      "gw-01/MyComponent/com.example.MyComponent",
    );
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

describe("sanitize (the normative UNS token sanitizer, D-U26)", () => {
  it("replaces the blacklist chars and traversal with '_'", () => {
    expect(sanitize("a/b\\c+d#e")).toBe("a_b_c_d_e");
    expect(sanitize("gw..01")).toBe("gw_01");
    expect(sanitize("gw 01")).toBe("gw 01"); // spaces are legal
    expect(sanitize("v1.2")).toBe("v1.2"); // single dots are legal
  });

  it("treats C0, DEL and C1 (U+0080-U+009F) as control characters (D-U26)", () => {
    expect(sanitize("ab")).toBe("a_b"); // C0
    expect(sanitize("ab")).toBe("a_b"); // DEL
    expect(sanitize("ab")).toBe("a_b"); // C1 NEL
    expect(sanitize("ab")).toBe("a_b"); // C1 upper bound
    expect(sanitize("a b")).toBe("a b"); // U+00A0 NBSP is NOT a control char
    expect(isIsoControl(0x85)).toBe(true);
    expect(isIsoControl(0xa0)).toBe(false);
  });
});
