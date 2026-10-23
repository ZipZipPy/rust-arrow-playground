import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

// `npm run dev` proxies /api to the local Rust server; API_PROXY points it elsewhere.
const apiProxy = { '/api': { target: process.env.API_PROXY ?? 'http://localhost:8080', changeOrigin: true } };

export default defineConfig({
  plugins: [react()],
  server: { port: 5173, proxy: apiProxy },
  preview: { port: 4173, proxy: apiProxy },
  // parquet-wasm locates its .wasm relative to its own module; keep it out of
  // dependency pre-bundling so dev and build load it the same way.
  optimizeDeps: { exclude: ['parquet-wasm'] },
  // One page that needs apache-arrow up front (most of the main chunk); parquet-wasm is already split off.
  build: { outDir: 'dist', target: 'es2022', chunkSizeWarningLimit: 640 },
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
    testTimeout: 30_000,
  },
});
