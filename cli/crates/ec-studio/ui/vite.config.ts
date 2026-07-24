import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Base "" so the embedded bundle is served from the Studio server's root with relative asset URLs.
// The dev server proxies /api and /healthz to a locally running `studio serve` (default :8788).
export default defineConfig({
  plugins: [react()],
  base: "",
  build: { outDir: "dist", emptyOutDir: true },
  server: {
    proxy: {
      "/api": { target: "http://127.0.0.1:8788" },
      "/healthz": { target: "http://127.0.0.1:8788" },
    },
  },
});
