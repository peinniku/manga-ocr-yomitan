import preact from '@preact/preset-vite';
import { defineConfig } from 'vite';

// Served by the desktop app at /app/ (and to the phone through `tailscale serve`).
export default defineConfig({
  base: '/app/',
  plugins: [preact()],
  build: { outDir: 'dist', emptyOutDir: true, target: 'safari16' },
  server: {
    proxy: { '/api': 'http://127.0.0.1:7331', '/health': 'http://127.0.0.1:7331' },
  },
});
