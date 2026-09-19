import { createRoot } from 'react-dom/client';
import { TrajectoryTimeline } from '../../src/app/trajectory/TrajectoryTimeline';
import { traceRecord, traceTool } from '../trace-fixture';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/reset.css';

const noop = () => {};
const boundaryLabel = () => undefined;
function Fixture({ bridge }: { bridge: boolean }) {
  const request = traceRecord(0);
  request.timing.duration_ms = '9000';
  request.timing.ended_at = '2026-09-15T00:00:09Z';
  request.request!.generation = {
    timeline: bridge ? { dispatch_ms: '400', first_output_ms: '720', last_output_ms: '1920', terminal_ms: '2000' } : null,
    ttft_ms: '320', generation_ms: '1280', terminal_ms: '1600', output_tokens_per_second: 93.75,
  };
  const reference = traceTool(1, { timing: { ...request.timing } });
  return <section aria-label={bridge ? 'Measured bridge' : 'Missing bridge'}>
    <h1>{bridge ? '400 ms preparation, 320 ms TTFT, 1280 ms generation' : 'Numeric TTFT without a bridge'}</h1>
    <TrajectoryTimeline records={[request, reference]} mode="duration" range={null}
      selectedId={null} searchMatches={null} onRangeChange={noop} onSelect={noop} boundaryLabel={boundaryLabel} />
  </section>;
}
createRoot(document.getElementById('root')!).render(<><Fixture bridge /><Fixture bridge={false} /></>);
