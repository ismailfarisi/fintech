import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// In dev, proxy the API (and its WebSocket) to the Rust backend. The backend
// must list this dev origin in ALLOWED_ORIGINS (see .env.example).
const backend = process.env.BACKEND_URL ?? 'http://localhost:8080'

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': { target: backend, ws: true },
    },
  },
})
