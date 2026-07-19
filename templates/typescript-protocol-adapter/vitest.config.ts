import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["test/**/*.test.ts"],
    environment: "node",
    coverage: {
      provider: "v8",
      include: ["src/**"],
      exclude: ["src/main.ts"],
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
