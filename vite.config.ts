import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

// https://vite.dev/config/
export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
  ],
  // Whisper 전사 워커는 transformers.js / onnxruntime-web 을 동적 import 로 끌어온다.
  // 기본값인 iife 워커 번들은 코드 분할과 dynamic import 를 못 해 그 자리에서 깨진다.
  worker: {
    format: 'es',
  },
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    watch: {
      ignored: ['**/src-tauri/**'],
    },
  },
});
