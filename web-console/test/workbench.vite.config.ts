import config from '../vite.config';
import { workbenchFixture } from './e2e/workbench-host';
export default { ...config, plugins: [workbenchFixture(), ...(config.plugins ?? [])] };
