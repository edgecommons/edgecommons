/**
 * Unit tests for the facade value types (DESIGN-class-facades) — {@link Channel}, {@link Quality},
 * {@link Severity}, {@link SignalUpdateBuilder} — the parts every mirror replicates verbatim.
 * Mirrors the Java `FacadeValueTypesTest`.
 */
import { describe, expect, it } from "vitest";

import { Channel } from "../src/facades/channel";
import { Quality, qualityFromWire } from "../src/facades/quality";
import { Severity, severityFromWire } from "../src/facades/severity";
import { SignalUpdateBuilder } from "../src/facades/signal_update";
import { effectiveSignalPath } from "../src/facades/signal_update";
import { GgError } from "../src/errors";

describe("Channel", () => {
  it("fromConfig parses every recognized form", () => {
    expect(Channel.fromConfig("local")).toEqual(Channel.LOCAL);
    expect(Channel.fromConfig("LOCAL")).toEqual(Channel.LOCAL);
    expect(Channel.fromConfig("northbound")).toEqual(Channel.NORTHBOUND);
    expect(Channel.fromConfig("iotcore")).toEqual(Channel.NORTHBOUND);
    expect(Channel.fromConfig("iot_core")).toEqual(Channel.NORTHBOUND);
    expect(Channel.fromConfig("stream:hot")).toEqual(Channel.stream("hot"));
  });

  it("fromConfig yields undefined for absent or unrecognized", () => {
    expect(Channel.fromConfig(undefined)).toBeUndefined();
    expect(Channel.fromConfig(null)).toBeUndefined();
    expect(Channel.fromConfig("")).toBeUndefined();
    expect(Channel.fromConfig("   ")).toBeUndefined();
    expect(Channel.fromConfig("bogus")).toBeUndefined();
    expect(Channel.fromConfig("stream:"), "an empty stream name is not a valid channel").toBeUndefined();
  });

  it("stream() rejects an empty name", () => {
    expect(() => Channel.stream("")).toThrow(GgError);
  });

  it("structural equality and the config-string round trip", () => {
    expect(Channel.LOCAL).toEqual(Channel.LOCAL);
    expect(Channel.stream("hot")).toEqual(Channel.stream("hot"));
    expect(Channel.stream("hot")).not.toEqual(Channel.stream("cold"));
    expect(Channel.LOCAL).not.toEqual(Channel.NORTHBOUND);
    expect(Channel.toConfigString(Channel.LOCAL)).toBe("local");
    expect(Channel.toConfigString(Channel.NORTHBOUND)).toBe("northbound");
    expect(Channel.toConfigString(Channel.stream("hot"))).toBe("stream:hot");
  });
});

describe("Quality / Severity", () => {
  it("quality wire tokens are UPPERCASE", () => {
    expect(Quality.Good).toBe("GOOD");
    expect(Quality.Bad).toBe("BAD");
    expect(Quality.Uncertain).toBe("UNCERTAIN");
    expect(qualityFromWire("GOOD")).toBe(Quality.Good);
    expect(qualityFromWire("good"), "wire tokens are UPPERCASE").toBeUndefined();
    expect(qualityFromWire("nope")).toBeUndefined();
  });

  it("severity wire tokens are lowercase", () => {
    expect(Severity.Critical).toBe("critical");
    expect(severityFromWire("info")).toBe(Severity.Info);
    expect(severityFromWire("INFO"), "wire tokens are lowercase").toBeUndefined();
  });
});

describe("SignalUpdateBuilder", () => {
  it("build() collects every field; unset fields stay undefined", () => {
    const address = { ns: 2 };
    const update = new SignalUpdateBuilder("sig-1").name("Signal One").address(address).addSample(1.0).build();

    expect(update.signalId).toBe("sig-1");
    expect(update.signalName).toBe("Signal One");
    expect(update.signalAddress).toEqual(address);
    expect(effectiveSignalPath(update), "signalPath defaults to signalId").toBe("sig-1");
    expect(update.via).toBeUndefined();
    expect(update.samples).toHaveLength(1);
    expect(update.device).toBeUndefined();
  });

  it("signalPath()/via() override the effective path/channel", () => {
    const base = new SignalUpdateBuilder("sig-1").addSample(1.0).build();
    const withPath = new SignalUpdateBuilder("sig-1").signalPath("a/b").via(Channel.NORTHBOUND).addSamples(base.samples).build();

    expect(effectiveSignalPath(withPath)).toBe("a/b");
    expect(withPath.via).toEqual(Channel.NORTHBOUND);
  });

  it("device(adapter, instance, endpoint) and device(object) both set the device block", () => {
    const a = new SignalUpdateBuilder("s").device("opcua", "kep1", "opc.tcp://host:4840").build();
    expect(a.device).toEqual({ adapter: "opcua", instance: "kep1", endpoint: "opc.tcp://host:4840" });

    const b = new SignalUpdateBuilder("s").device({ adapter: "modbus" }).build();
    expect(b.device).toEqual({ adapter: "modbus" });
  });

  it("a detached builder's publish() throws", async () => {
    const detached = new SignalUpdateBuilder("temp").addSample(1.0);
    await expect(detached.publish()).rejects.toThrow(GgError);
  });
});
