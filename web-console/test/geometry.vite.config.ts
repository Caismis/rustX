import config from '../vite.config';
import { fileURLToPath } from 'node:url';
// A separate production build of the same source owners and viewer. This never
// enters the shipped application or changes the pinned browser authority.
export default { ...config, build: { outDir: fileURLToPath(new URL('../../work/geometry-production/', import.meta.url)), emptyOutDir: true,
  rollupOptions: { input: fileURLToPath(new URL('./fixtures/geometry.html', import.meta.url)) } } };
