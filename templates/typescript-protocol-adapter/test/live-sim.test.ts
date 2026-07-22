/**
 * # The live-sim integration suite
 *
 * Every other test in this project runs against the in-process simulator with no external
 * dependency. This suite is different: it is gated on `EC_LIVE_SIM` and **skipped** — not failing —
 * in a normal `npm test` run, so the scaffold-build CI gate stays green with no simulator running.
 *
 * Point `EC_LIVE_SIM` at whatever your backend's `connect()` needs and run:
 *
 * ```bash
 * EC_LIVE_SIM=sim://device-1 npm test
 * ```
 *
 * The built-in simulator only needs a non-empty endpoint string, so this suite is runnable today
 * with no extra infrastructure. Once you replace `src/device.ts`'s `SimBackend` with a real
 * protocol, update this suite to build your real backend (via `backendFor`) and point
 * `EC_LIVE_SIM` at a reachable device or a protocol simulator — mirroring how the sibling reference
 * adapters gate their own live suites on a permanent or on-demand simulator (the modbus reference
 * adapter's `ggcommons-modbus-sim` container, or the EtherNet/IP adapter's cpppo/OpENer suites).
 */
import { describe, expect, it } from "vitest";

import { Quality, backendFor } from "../src/device";

const liveEndpoint = process.env.EC_LIVE_SIM;

describe.skipIf(!liveEndpoint)("live sim integration", () => {
  it("connects, polls once, and returns readings with quality", async () => {
    const backend = backendFor("sim");
    if (!backend) throw new Error("the sim backend must be registered");

    const session = await backend.connect({ endpoint: liveEndpoint as string });
    try {
      const readings = await session.readSignals();
      expect(readings.length).toBeGreaterThan(0);

      // Every reading carries a normalized quality — the structural guarantee the whole southbound
      // contract depends on (see docs/explanation.md#quality-is-not-optional).
      for (const r of readings) {
        expect(Object.values(Quality)).toContain(r.quality);
      }

      const ok = readings.find((r) => r.quality === Quality.Good);
      expect(ok).toBeDefined();
    } finally {
      await session.close();
    }
  });
});
