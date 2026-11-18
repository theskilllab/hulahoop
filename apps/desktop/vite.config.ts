import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The server is a local process; the webview talks to it over HTTP, so the dev
// server must not try to proxy anything.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 5183, strictPort: true },
  build: { outDir: "dist", target: "safari15" },
});
