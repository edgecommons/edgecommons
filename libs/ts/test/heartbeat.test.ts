import { describe, it, expect } from "vitest";

import { HeartbeatMonitor } from "../src/heartbeat";
import type { Measures } from "../src/config/model";

function measures(overrides: Partial<Measures>): Measures {
  return {
    cpu: false,
    memory: false,
    disk: false,
    threads: false,
    files: false,
    fds: false,
    ...overrides,
  };
}

describe("HeartbeatMonitor.getStats", () => {
  it("returns only enabled measures with the nested shape", () => {
    const monitor = new HeartbeatMonitor(measures({ memory: true }));
    const stats = monitor.getStats();

    expect(Object.keys(stats)).toEqual(["memory"]);
    const mem = stats.memory as Record<string, unknown>;
    expect(typeof mem.memory_usage).toBe("number");
    expect(mem.memory_usage as number).toBeGreaterThan(0);
  });

  it("omits disabled measures", () => {
    const monitor = new HeartbeatMonitor(measures({}));
    expect(monitor.getStats()).toEqual({});
  });

  it("includes multiple enabled measures with nested keys", () => {
    const monitor = new HeartbeatMonitor(measures({ cpu: true, memory: true }));
    const stats = monitor.getStats();
    expect(Object.keys(stats).sort()).toEqual(["cpu", "memory"]);
    expect(stats.cpu).toHaveProperty("cpu_usage");
    expect(stats.memory).toHaveProperty("memory_usage");
  });

  it("cpu_usage is 0 on the first sample (no baseline)", () => {
    const monitor = new HeartbeatMonitor(measures({ cpu: true }));
    const stats = monitor.getStats();
    expect((stats.cpu as Record<string, number>).cpu_usage).toBe(0);
  });
});
