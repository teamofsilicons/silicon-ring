import { defineConfig } from 'vite';
import solid from 'vite-plugin-solid';
import { fileURLToPath } from 'node:url';
export default defineConfig({
  plugins: [solid()],
  server: { port: 1420, strictPort: true },
  build: { target: 'es2022', rollupOptions: { input: { index: fileURLToPath(new URL('index.html', import.meta.url)), nativeLogin: fileURLToPath(new URL('native-login.html', import.meta.url)) } } },
  clearScreen: false,
});
