import config from '../vite.config';
import { workbenchFixture } from './e2e/workbench-host';
// Scan every browser fixture before serving modules, so lazy fixture imports
// cannot invalidate an already loaded dependency URL during acceptance.
export default { ...config, optimizeDeps: { ...config.optimizeDeps, entries: ['index.html', 'test/fixtures/*.html'] }, plugins: [workbenchFixture(), ...(config.plugins ?? [])] };
