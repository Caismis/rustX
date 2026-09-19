import { createRoot } from 'react-dom/client';
import { Trajectory } from '../../src/app/trajectory/Trajectory';
import { completeTraceDetail, replaceTrace, selectTrace } from '../../src/client/trace';
import { toolDetail, traceTool } from '../trace-fixture';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/reset.css';

const locator = '/private/rustx-managed-output/example/tasks/result.output';
const detail = toolDetail(0);
detail.tool!.result!.managed_output = {
  complete: false, available: true, locator,
  diagnostic: { text: `cannot append ${locator}: recorded I/O failure`, truncated: false },
};
let cache = replaceTrace({ records: [traceTool(0)], next_cursor: null });
cache = completeTraceDetail(selectTrace(cache, 'trace:0'), 'trace:0', cache.epoch, detail);
const noop = () => {};
createRoot(document.getElementById('root')!).render(
  <Trajectory cache={cache} loadEarlier={noop} latest={noop} onSelect={noop} onLoadDetail={noop} />,
);
