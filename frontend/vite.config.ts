import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// In dev, proxy API + WebSocket traffic to the Rust backend.
const backend = process.env.BACKEND_URL ?? 'http://localhost:8080'

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': backend,
      '/ws': { target: backend, ws: true },
    },
  },
})
