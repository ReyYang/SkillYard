import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Tauri 开发服务器固定端口，生产构建仍只加载打包后的静态资源。
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    restoreMocks: true,
  },
});
