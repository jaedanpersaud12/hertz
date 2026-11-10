import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig, loadEnv } from 'vite'

export default defineConfig(({ mode }) => {
  const port = Number(loadEnv(mode, process.cwd(), '').PORT ?? 5174)
  return {
    plugins: [react(), tailwindcss()],
    server: { port, strictPort: true, host: '127.0.0.1', proxy: { '/api': `http://127.0.0.1:${port + 1}` } },
  }
})
