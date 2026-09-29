import { defineConfig } from "vite";
import { resolve } from "node:path";
export default defineConfig({
  base: "./",
  server: { host: "127.0.0.1", port: 5173, strictPort: true },
  build: {
    sourcemap: true,
    rollupOptions: {
      input: {
        main: resolve(__dirname, "index.html"),
        mascot: resolve(__dirname, "mascot.html"),
      },
      output: {
        manualChunks: { terminal: ["@xterm/xterm", "@xterm/addon-fit"] },
      },
    },
  },
});
