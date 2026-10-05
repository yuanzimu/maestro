import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 固定端口（tauri.conf.json devUrl 指向这里）
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: "es2021" },
});
