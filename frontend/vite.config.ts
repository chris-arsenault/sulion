import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const apiTarget = process.env.SULION_API_TARGET ?? "http://localhost:8080";
const wsTarget =
  process.env.SULION_WS_TARGET ??
  apiTarget.replace(/^http/i, (value) => (value.toLowerCase() === "https" ? "wss" : "ws"));

export default defineConfig({
  plugins: [react()],
  worker: {
    format: "es",
  },
  server: {
    proxy: {
      "/api": apiTarget,
      "/ws": { target: wsTarget, ws: true },
      "/health": apiTarget,
    },
  },
});
