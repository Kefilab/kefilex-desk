import { defineConfig } from 'vite'
import { readFileSync } from 'node:fs'

const pkg = JSON.parse(readFileSync('./package.json', 'utf8')) as { version: string }

// Vite config tailored for a Tauri WebView frontend.
//   - Fixed dev port (1420) — must match tauri.conf.json's devUrl.
//   - Disable host check so Tauri's bundled WebView can connect.
//   - Inject the package.json version as VITE_APP_VERSION so the UI
//     shows the same string the binary reports on heartbeat.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: 'localhost',
  },
  build: {
    target: ['es2022', 'chrome105', 'safari14'],
    minify: 'esbuild',
    sourcemap: true,
  },
  define: {
    'import.meta.env.VITE_APP_VERSION': JSON.stringify(pkg.version),
  },
})
