import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  base: './',
  clearScreen: false,
  build: { outDir: 'dist' },
  server: { port: 1420, strictPort: true }
})
