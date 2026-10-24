import { useState } from 'react';
import type { TraceContentBlock } from '../../../protocol/app-server/v57';
import { createRoot } from 'react-dom/client';
import { Tool } from '../../src/app/agent/Tool';
import { ToolInspectionContext } from '../../src/app/agent/tool-inspection';
import { TrajectoryInspector } from '../../src/app/trajectory/TrajectoryInspector';
import type { TrajectoryFacet } from '../../src/app/trajectory/layout';
import { nativeToolCases } from '../native-tool-cases';
import { traceTool, toolDetail } from '../trace-fixture';
import { localeController } from '../../src/locale/controller';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/gradient-shadow-text.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/reset.css';

localeController.setLocale(new URLSearchParams(location.search).get('locale') === 'zh' ? 'zh' : 'en');
function Fixture() {
  const [selected, setSelected] = useState<string>(), [facet, setFacet] = useState<TrajectoryFacet>('overview');
  const tool = nativeToolCases.find(tool => tool.call_id === selected);
  const record = traceTool(1), detail = toolDetail(1);
  if (tool?.state.type === 'settled') {
    record.tool = { ...record.tool!, tool_id: tool.tool_id ?? null, name: tool.name };
    detail.tool = { ...detail.tool!, tool_id: tool.tool_id ?? '', name: tool.name, arguments: { value: JSON.parse(tool.state.arguments), truncated: false }, source: null,
      result: { ...detail.tool!.result!, blocks: (tool.state.result.content ?? []).flatMap((block): TraceContentBlock[] => block.type === 'json' ? [{ type: 'json' as const, value: { value: block.value, truncated: false } }]
        : block.type === 'text' ? [{ type: 'text' as const, text: { text: block.text, truncated: false } }] : []) } };
  }
  return <main style={{ maxWidth: 760, margin: '24px auto', padding: 16 }}>
    <h1>Native tools</h1>
    <ToolInspectionContext value={async locator => { setFacet('overview'); setSelected(locator.call_id ?? undefined); }}>
      <section aria-label="Native tools">{nativeToolCases.map(tool => <Tool tool={tool} key={tool.call_id}/>)}</section>
    </ToolInspectionContext>
    {tool && <aside aria-label="Native tool inspection" style={{ marginTop: 24, border: '1px solid var(--dsw-alias-border-l1)', borderRadius: 12, padding: 16 }}>
      <TrajectoryInspector item={{ type: 'RecordRow', record, display_key: record.id, owner_record_id: record.id, facet: 'overview', label: '', preview: '' }} detail={detail} facet={facet} onFacet={setFacet} onClose={() => setSelected(undefined)} onLoadDetail={() => {}} records={[record]} executions={new Map()} requestNumbers={new Map()} completeHistory onOpen={() => {}}/>
    </aside>}
  </main>;
}
createRoot(document.getElementById('root')!).render(<Fixture/>);
