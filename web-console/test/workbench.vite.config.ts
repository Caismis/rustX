import config from '../vite.config.ts';
import { workbenchFixture } from './e2e/workbench-host.ts';

export default {
  ...config,
  cacheDir: `${config.cacheDir}-workbench`,
  optimizeDeps: {
    ...config.optimizeDeps,
    // Scan the lazy editor before its form opens so its CodeMirror imports
    // cannot invalidate a dependency URL already served to the browser.
    entries: ['index.html', 'test/fixtures/*.html', 'src/presentation/primitives/JsonCodeView.tsx'],
  },
  plugins: [workbenchFixture(), ...(config.plugins ?? [])],
};
