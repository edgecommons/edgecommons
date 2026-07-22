import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["test/**/*.test.ts"],
    environment: "node",
    coverage: {
      provider: "v8",
      include: ["src/**"],
      exclude: [
        // The CLI bootstrap: parses argv, builds the runtime, installs the SIGTERM/SIGINT owner,
        // and hands off to App. Pure wiring over a live runtime — validated by the deploy paths.
        "src/main.ts",
        // The thin live-runtime seam: the App class runs one connect/poll/reconnect supervisor per
        // device and services each device's control channel. It needs a built EdgeCommons + a
        // messaging transport + real devices or the simulator + a clock to do anything, so it is
        // validated by the deploy paths and by test/live-sim.test.ts on real infra (the AGENTS.md
        // validation matrix), not a unit test. Every decision it delegates to — config parsing, the
        // reconnect backoff, the control mailbox, health/connectivity, the publish path (pollOnce),
        // and the per-message control handlers (handleControl/serveWhileDown) — lives in src/app.ts
        // and is covered by unit tests.
        "src/runtime.ts",
      ],
      // `cobertura` feeds a diff-coverage gate if you wire one up; `text` is the human-readable
      // summary printed in CI.
      reporter: ["text", "cobertura"],
      thresholds: {
        // The org's 90%-line coverage gate (all four languages, live-infra paths excluded — see
        // AGENTS.md). `test/live-sim.test.ts` is excluded from this run's *coverage numerator* by
        // virtue of self-skipping, not by file exclusion, so it never launders untested code.
        lines: 90,
      },
    },
  },
});
