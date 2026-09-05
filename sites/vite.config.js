import { defineConfig } from 'vite';
import { fileURLToPath } from 'node:url';

export default defineConfig({
  base: './',
  build: {
    rollupOptions: {
      input: {
        home: fileURLToPath(new URL('./index.html', import.meta.url)),
        glasshouse: fileURLToPath(new URL('./glasshouse/index.html', import.meta.url)),
        pane: fileURLToPath(new URL('./pane/index.html', import.meta.url)),
      },
    },
  },
});
