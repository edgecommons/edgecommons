import { describe, expect, it } from "vitest";

import { ConnectionConfig, DeviceError, Quality, SimBackend, SimSession, backendFor } from "../src/device";

const conn = (endpoint: string): ConnectionConfig => ({ endpoint });

describe("the sim backend", () => {
  it("connects and reads", async () => {
    const session = await new SimBackend().connect(conn("sim://device"));
    const readings = await session.readSignals();
    expect(readings).toHaveLength(2);
    expect(readings[0].signalId).toBe("temperature-1");
    expect(readings[0].quality).toBe(Quality.Good);
  });

  it("publishes a failed read as BAD quality rather than omitting it", async () => {
    // The signal is still reported — with BAD quality and the native code — because a signal that
    // silently vanishes is indistinguishable from one that is not changing.
    const session = await new SimBackend().connect(conn("sim://device"));
    const readings = await session.readSignals();
    const bad = readings.find((r) => r.signalId === "pressure-1");
    expect(bad?.quality).toBe(Quality.Bad);
    expect(bad?.qualityRaw).toBe("SENSOR_FAULT");
  });

  it("treats a misconfiguration as permanent so the supervisor does not hammer it", async () => {
    await expect(new SimBackend().connect(conn(""))).rejects.toThrow(/no endpoint/);
    const error = await new SimBackend().connect(conn("")).catch((e: unknown) => e);
    expect(DeviceError.isTransient(error)).toBe(false);
  });

  it("advances its readings", async () => {
    const session = await new SimBackend().connect(conn("sim://device"));
    const first = (await session.readSignals())[0].value;
    const second = (await session.readSignals())[0].value;
    expect(first).not.toBe(second);
  });

  it("closes idempotently", async () => {
    const session = (await new SimBackend().connect(conn("sim://device"))) as SimSession;
    await session.close();
    await session.close();
    expect(session.isClosed()).toBe(true);
  });

  it("resolves its backend by kind, and only its own", () => {
    expect(backendFor("sim")?.kind).toBe("sim");
    expect(backendFor("modbus")).toBeUndefined();
  });
});
