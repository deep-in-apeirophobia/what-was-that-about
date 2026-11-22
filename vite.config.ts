/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { sentryVitePlugin } from "@sentry/vite-plugin";

const host = process.env.TAURI_DEV_HOST;
const debug = !!process.env.TAURI_ENV_DEBUG;
// CI release builds: upload hidden source maps to Sentry (matched by debug ID), then delete
// them so they aren't bundled into the app. Off without a token, so local builds are unchanged.
const uploadSourceMaps = !debug && !!process.env.SENTRY_AUTH_TOKEN;

// https://v2.tauri.app/start/frontend/vite/
export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    uploadSourceMaps &&
      sentryVitePlugin({
        org: "cactus-66",
        project: "what-was-that-about",
        authToken: process.env.SENTRY_AUTH_TOKEN,
        telemetry: false,
        // The app reports its release from Rust; don't create or inject one here.
        release: { create: false, inject: false },
        sourcemaps: { filesToDeleteAfterUpload: ["dist/**/*.map"] },
      }),
  ],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**", "**/crates/**", "**/target/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari15",
    minify: debug ? false : "oxc",
    sourcemap: debug ? true : uploadSourceMaps ? "hidden" : false,
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
