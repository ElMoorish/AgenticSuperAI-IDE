import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev port and does not want the screen cleared.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, host: "localhost" },
  // The ~4 MB Monaco chunk (and the ts/css/html/json workers) load lazily on the first opened file;
// the shell itself stays under ~200 kB, so the default 500 kB warning is raised for that one chunk.
build: {
    target: "chrome105",
    outDir: "dist",
    emptyOutDir: true,
    chunkSizeWarningLimit: 4200,
    // two pages: the IDE and the small always-on-top approval window
    rollupOptions: { input: { main: "index.html", overlay: "overlay.html" } },
  },
});
