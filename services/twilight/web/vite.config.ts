import react from '@vitejs/plugin-react'
import { msw } from 'msw/vite'
import { defineConfig } from 'vitest/config'

export default defineConfig({
  plugins: [react(), msw()],
  server: {
    proxy: {
      '/api': {
        target: process.env.TWILIGHT_API_URL ?? 'http://127.0.0.1:8080',
      },
    },
  },
  build: {
    outDir: 'dist',
    sourcemap: true,
    rolldownOptions: {
      output: {
        codeSplitting: {
          groups: [
            { name: 'mantine', test: /node_modules[\\/]@mantine[\\/]/ },
            { name: 'codemirror', test: /node_modules[\\/](@codemirror|@lezer)[\\/]/ },
          ],
        },
      },
    },
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    css: false,
    testTimeout: 20000,
  },
})
