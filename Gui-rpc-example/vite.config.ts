import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 开发时使用固定端口，WebView 通过 devUrl 加载
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "es2021",
  },
});
