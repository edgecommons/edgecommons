import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["test/**/*.test.ts"],
    environment: "node",
    coverage: {
      provider: "v8",
      include: ["src/**"],
      // Excluded from coverage — no testable logic, or validated out-of-band:
      //  - runnable process-entry harnesses: interop_node (cross-language IPC driver) and the
      //    *_verify smoke tests (gg/cw/ipc), exercised on real infra rather than unit tests;
      //  - barrel re-export files (index.ts) and type-only declaration modules (types.ts).
      exclude: [
        "src/interop_node.ts",
        "src/*_verify.ts",
        "src/index.ts",
        "src/**/index.ts",
        "src/**/types.ts",
      ],
      // `cobertura` (coverage/cobertura-coverage.xml) feeds the diff-coverage gate; `text`
      // is the human-readable summary.
      reporter: ["text", "cobertura"],
      thresholds: {
        statements: 92,
        lines: 92,
        functions: 85,
        branches: 80,
      },
    },
  },
});
