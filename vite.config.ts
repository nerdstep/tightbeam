import { fileURLToPath } from 'node:url'

import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// Tauri loads the dev server in development and ../dist in builds (see tauri.conf.json).
export default defineConfig({
  root: 'ui',
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { '@': fileURLToPath(new URL('./ui/src', import.meta.url)) },
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  build: {
    outDir: '../dist',
    emptyOutDir: true,
    // WebView2 on Windows, WebKit on macOS. Tailwind v4 needs Safari 16.4+.
    target: ['es2022', 'chrome111', 'safari16.4'],
  },
})
