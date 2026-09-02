import { fileURLToPath, URL } from 'node:url'

import tailwindcss from '@tailwindcss/vite'
import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vite'

export default defineConfig({
  plugins: [vue(), tailwindcss()],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  // Tauri 端口约定：固定避免 devUrl 漂移；仅监听回环
  server: {
    port: 5178,
    strictPort: true,
    host: '127.0.0.1',
  },
  // 生产构建产物供 Tauri 壳打包
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'chrome110',
  },
  clearScreen: false,
})
