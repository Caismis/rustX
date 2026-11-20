import { defaultClientConditions } from 'vite';
import config from '../vite.config.ts';
import { workbenchFixture } from './e2e/workbench-host.ts';
// Scan every browser fixture before serving modules, so lazy fixture imports
// cannot invalidate an already loaded dependency URL during acceptance.
export default {
  ...config,
  // Acceptance runs against the same browser dependency exports as the
  // production bundle, including Lit loaded through the A2UI catalog.
  resolve: { ...config.resolve, conditions: defaultClientConditions.map(condition => condition === 'development|production' ? 'production' : condition) },
  optimizeDeps: { ...config.optimizeDeps, entries: ['index.html', 'test/fixtures/*.html'] },
  plugins: [workbenchFixture(), ...(config.plugins ?? [])],
};
