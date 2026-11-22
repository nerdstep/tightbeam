import { preact } from '@preact/preset-vite'
import tailwindcss from '@tailwindcss/vite'
import { defineConfig } from 'vite'

// Tauri loads the dev server in development and ../dist in builds (see tauri.conf.json).
export default defineConfig({
  root: 'ui',
  plugins: [preact(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  build: {
    outDir: '../dist',
    emptyOutDir: true,
    // WebView2 on Windows, WebKit on macOS 11+.
    target: ['es2021', 'chrome105', 'safari14'],
  },
})
