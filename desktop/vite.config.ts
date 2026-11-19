/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed port in dev and loads the bundled dist/ in builds.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1421, strictPort: true },
  build: {
    target: "safari16",
    // Audio worklets must be files: the CSP (script-src 'self') blocks a
    // small one Vite would otherwise inline as a data: URL.
    assetsInlineLimit: (file) => (file.includes(".worklet.") ? false : undefined),
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    css: false,
  },
});
