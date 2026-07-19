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
        // AGENTS.md).
        lines: 90,
      },
    },
  },
});
