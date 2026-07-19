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
        // and hands off to App. Pure wiring over a live runtime — validated by the HOST/GG/K8s
        // deploy paths, not a unit test.
        "src/main.ts",
        // The thin live-runtime seam: the App class wires the edgecommons service handles together
        // and drives the infinite demo loop. It needs a built EdgeCommons + a messaging transport +
        // an interval clock to do anything, so it is validated by the deploy paths (the AGENTS.md
        // validation matrix), not a unit test. The decisions worth testing (the set-greeting verb,
        // the connectivity provider) live in src/app.ts and are covered there.
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
