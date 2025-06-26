import { defineConfig } from 'vite'

export default defineConfig({
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src/**"],
    },
  },
  clearScreen: false,
  envPrefix: ['VITE_', 'TAURI_'],
  build: {
    target: process.env.TAURI_PLATFORM == 'windows' ? 'chrome105' : 'safari13',
    minify: !process.env.TAURI_DEBUG ? 'esbuild' : false,
    sourcemap: !!process.env.TAURI_DEBUG,
  },
}) 