import { defineConfig } from 'vite';

export default defineConfig({
  root: 'ui',
  base: './',
  clearScreen: false,
  build: {
    outDir: '../ui-dist',
    emptyOutDir: true,
  },
  server: {
    host: '127.0.0.1',
    port: 5173,
    strictPort: true,
  },
});
