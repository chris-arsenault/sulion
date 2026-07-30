import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const apiTarget = process.env.SULION_API_TARGET ?? "http://localhost:8080";
const brokerTarget = process.env.SULION_BROKER_TARGET ?? "http://localhost:8081";
const wsTarget =
  process.env.SULION_WS_TARGET ??
  apiTarget.replace(/^http/i, (value) => (value.toLowerCase() === "https" ? "wss" : "ws"));

export default defineConfig({
  // amazon-cognito-identity-js pulls in Node's `buffer`, which reads `global`
  // at module init. The production build wraps CommonJS so this never bites,
  // but the dev server's esbuild pre-bundle leaves the bare reference — and
  // an unmounted React app renders as a blank page.
  define: { global: "globalThis" },
  plugins: [react()],
  worker: {
    format: "es",
  },
  server: {
    proxy: {
      "/api": apiTarget,
      "/broker": {
        target: brokerTarget,
        rewrite: (path) => path.replace(/^\/broker/, ""),
      },
      "/ws": { target: wsTarget, ws: true },
      "/health": apiTarget,
    },
  },
});
