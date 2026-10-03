import { defineConfig } from 'vite';
import solid from 'vite-plugin-solid';
export default defineConfig({
  plugins: [solid()],
  server: { port: 1420, strictPort: true },
  build: { target: 'es2022' },
  clearScreen: false,
});
