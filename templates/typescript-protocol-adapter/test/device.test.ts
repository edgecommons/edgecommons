import { describe, expect, it } from "vitest";

import {
  BaseDeviceSession,
  BrowseError,
  ConnectionConfig,
  DeviceError,
  Quality,
  Reading,
  SimBackend,
  SimSession,
  backendFor,
} from "../src/device";

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

  it("reads a named subset via the default readNamed (reads all and filters)", async () => {
    // The default reads all and filters — override it only if your protocol reads a subset more
    // cheaply.
    const session = await new SimBackend().connect(conn("sim://device"));
    const got = await session.readNamed(["temperature-1"]);
    expect(got).toHaveLength(1);
    expect(got[0].signalId).toBe("temperature-1");
    // An unknown id resolves to nothing (the command layer reports it as a BAD/no-data entry).
    expect(await session.readNamed(["nope"])).toHaveLength(0);
  });

  it("browses one page and then stops", async () => {
    const session = await new SimBackend().connect(conn("sim://device"));
    const page = await session.browse(undefined, 100);
    expect(page.entries).toHaveLength(2);
    expect(page.entries[0].id).toBe("temperature-1");
    expect(page.entries[0].typeName).toBe("REAL");
    expect(page.nextCursor).toBeUndefined(); // the sim's first page is its last
    // A cursor asks for the page after the last — empty.
    const page2 = await session.browse("x", 100);
    expect(page2.entries).toHaveLength(0);
  });

  it("advertises its inventory without connecting", () => {
    // `sb/signals` reads this — a config view, no device round-trip.
    const inv = new SimBackend().inventory(conn("sim://device"));
    expect(inv).toHaveLength(2);
    expect(inv[0].id).toBe("temperature-1");
    expect(inv[0].name).toBe("Ambient temperature");
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

describe("the default seam", () => {
  it("reports browse as unsupported by default", async () => {
    // A protocol with no discovery keeps the default — honest, not a fake empty page.
    class NoBrowse extends BaseDeviceSession {
      async readSignals(): Promise<Reading[]> {
        return [];
      }
      async writeSignal(): Promise<void> {}
    }
    const s = new NoBrowse();
    const error = await s.browse(undefined, 10).catch((e: unknown) => e);
    expect(BrowseError.isBrowseError(error)).toBe(true);
    expect((error as BrowseError).reason).toBe("UNSUPPORTED");
  });
});
