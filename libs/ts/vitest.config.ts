import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["test/**/*.test.ts"],
    environment: "node",
    coverage: {
      provider: "v8",
      include: ["src/**"],
      // Only the two process-entry harnesses are excluded: they are thin runnable
      // wrappers validated out-of-band (interop_node is the cross-language IPC interop
      // driver; ipc_verify is the on-device nucleus smoke test). Everything else under
      // src/** is covered by the suite.
      exclude: ["src/interop_node.ts", "src/ipc_verify.ts"],
      reporter: ["text"],
      thresholds: {
        statements: 90,
        lines: 90,
        functions: 85,
        branches: 80,
      },
    },
  },
});
