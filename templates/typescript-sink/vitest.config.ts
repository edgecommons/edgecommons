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
        // The thin live-runtime seam: the App class wires the edgecommons handles together, builds
        // each destination, subscribes each sink, and hands every message to the delivery ladder.
        // It needs a built EdgeCommons + a messaging transport + a clock to do anything, so it is
        // validated by the deploy paths (the AGENTS.md validation matrix), not a unit test. Every
        // testable piece it drives — sink parsing, the retry backoff, the stable key, the delivery
        // ladder (deliverWithRetry), per-destination connectivity (src/app.ts) and the destination
        // backends (src/dest.ts) — is covered by unit tests.
        "src/runtime.ts",
      ],
      // `cobertura` feeds a diff-coverage gate if you wire one up; `text` is the human-readable
      // summary printed in CI.
      reporter: ["text", "cobertura"],
      thresholds: {
        // The org's 90%-line coverage gate (all four languages, live-infra paths excluded — see
        // AGENTS.md).
        lines: 90,
      },
    },
  },
});
