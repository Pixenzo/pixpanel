import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Geliştirme sırasında API istekleri test makinesindeki panele yönlendirilir.
const devTarget = process.env.CAPSULE_DEV_TARGET ?? "https://192.168.1.25:8443";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    proxy: {
      "/api": { target: devTarget, secure: false, changeOrigin: true, ws: true },
    },
  },
  build: { outDir: "dist", emptyOutDir: true, chunkSizeWarningLimit: 900 },
});
