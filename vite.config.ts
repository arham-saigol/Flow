import { defineConfig, loadEnv } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), "");
  const allowedHosts = env.VITE_ALLOWED_HOSTS
    ?.split(",")
    .map((host) => host.trim())
    .filter(Boolean);

  return {
    plugins: [react()],
    clearScreen: false,
    server: {
      host: env.VITE_DEV_HOST || "127.0.0.1",
      allowedHosts,
      port: 1420,
      strictPort: true,
      watch: {
        ignored: ["**/src-tauri/**"],
      },
    },
    envPrefix: ["VITE_", "TAURI_"],
    build: {
      target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
      minify: !process.env.TAURI_ENV_DEBUG ? "esbuild" : false,
      sourcemap: !!process.env.TAURI_ENV_DEBUG,
    },
  };
});
