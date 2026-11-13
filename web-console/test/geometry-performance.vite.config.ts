import config from '../vite.config.ts';
import { fileURLToPath } from 'node:url';

// Production modules and their actual browser fixtures, independent of the
// application bundle and the default screenshot lane's development server.
export default {
  ...config,
  build: {
    target: 'esnext', outDir: 'dist/geometry-performance', emptyOutDir: true,
    rollupOptions: { input: {
      geometry: fileURLToPath(new URL('./fixtures/geometry.html', import.meta.url)),
      foundation: fileURLToPath(new URL('./fixtures/foundation.html', import.meta.url)),
    } },
  },
};
