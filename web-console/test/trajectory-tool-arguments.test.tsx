import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CellContent, listText } from '../src/app/trajectory/TrajectoryCell';
import { TrajectoryInspector } from '../src/app/trajectory/TrajectoryInspector';
import { modelToolArgumentPreview } from '../src/app/trajectory/tool-arguments';
import { projectTrajectory, trajectoryItems, isInspectable, type InspectableDisplayItem } from '../src/app/trajectory/layout';
import { searchItems } from '../src/app/trajectory/search';
import { translator } from '../src/locale/translation';
import { toolDetail, traceTool } from './trace-fixture';

const tx = translator('en');
afterEach(cleanup);
const preview = (text: string, truncated = false) => ({ text, truncated });

it('keeps runtime metadata out of semantic tool cells and search while preserving raw detail', () => {
  const envelope = { arguments: { command: 'ls -la' }, execution_mode: 'foreground', elevation_reason: 'runtime escalation only' };
  const raw = JSON.stringify(envelope), record = traceTool(1), detail = toolDetail(1);
  record.tool!.arguments = preview(raw);
  detail.tool!.arguments = { value: envelope, truncated: false };
  detail.tool!.source = null;
  const frozen = JSON.stringify({ record, detail });
  const projection = projectTrajectory(tx, [record]);
  const item = trajectoryItems(tx, projection).find(isInspectable)!;
  expect(listText(tx, item)).toBe('bash {"command":"ls -la"}');
  const cell = render(<CellContent item={item}/>);
  expect(cell.container.textContent).toBe('bash{"command":"ls -la"}→ls -la');
  expect(cell.container.textContent).not.toContain('execution_mode');
  expect(cell.container.textContent).not.toContain('runtime escalation only');
  expect(searchItems(projection, 'ls -la')?.size).toBe(1);
  expect(searchItems(projection, 'execution_mode')?.size).toBe(0);
  expect(searchItems(projection, 'runtime escalation only')?.size).toBe(0);
  cell.unmount();
  render(<TrajectoryInspector item={item} facet="input" detail={detail} records={[record]} onFacet={vi.fn()} onLoadDetail={vi.fn()} onClose={vi.fn()} onOpen={vi.fn()} executions={new Map()} requestNumbers={new Map()} completeHistory/>);
  expect(screen.getByRole('tree', { name: 'Payload JSON' }).textContent).toContain('execution_mode');
  expect(screen.getByText('"runtime escalation only"')).toBeTruthy();
  expect(JSON.stringify({ record, detail })).toBe(frozen);
});

it.each([
  ['{"arguments":{"command":"ls -la"},"execution_mode":"foreground"}', false, '{"command":"ls -la"}'],
  ['{"elevation_reason":"outer reason","arguments":{"command":"bounded input', true, '{"command":"bounded input'],
  ['{"execution_mode":"background","arguments":{"n":18446744073709551615,"decimal":1.0000,"exponent":1e400}}', false, '{"n":18446744073709551615,"decimal":1.0000,"exponent":1e400}'],
  ['{"arguments":{"arguments":{"value":1},"execution_mode":"business mode","elevation_reason":"business reason"},"elevation_reason":"outer reason"}', false, '{"arguments":{"value":1},"execution_mode":"business mode","elevation_reason":"business reason"}'],
  ['{"arguments":{"command":"done"},"elevation_reason":"truncated outer', true, '{"command":"done"}'],
  ['{"elevation_reason":"truncated before business', true, undefined],
  ['{"command":"flat invocation"}', false, undefined],
  ['{"arguments":null}', false, undefined],
  ['{"arguments":[]}', false, undefined],
  ['{"arguments":{"command":"malformed complete', false, undefined],
  ['{"arguments":{"first":1},"arguments":{"last":2}}', false, undefined],
  ['{"arguments":{"first":1},"arguments":{"last":', true, undefined],
] as const)('projects exactly one business node from %s', (raw, truncated, business) => {
  expect(modelToolArgumentPreview(preview(raw, truncated))).toBe(business);
  const record = traceTool(2);
  record.tool!.arguments = preview(raw, truncated);
  const item: InspectableDisplayItem = { type: 'RecordRow', record, display_key: record.id, owner_record_id: record.id, facet: 'overview', label: '', preview: '' };
  const text = listText(tx, item);
  expect(text).toBe(business === undefined ? 'bash' : `bash ${business}`);
  const ui = render(<CellContent item={item}/>);
  expect(ui.container.querySelector('[class*="toolCallPayload"]')?.textContent).toBe(business);
  expect(record.tool!.arguments!.text).toBe(raw);
});

it('searches same-named business fields without exposing outer metadata', () => {
  const raw = '{"arguments":{"arguments":{"query":"nested business"},"execution_mode":"business mode","elevation_reason":"business reason"},"execution_mode":"background","elevation_reason":"runtime metadata"}';
  const record = traceTool(3);
  record.tool!.arguments = preview(raw);
  const projection = projectTrajectory(tx, [record]);
  for (const query of ['nested business', 'business mode', 'business reason']) expect(searchItems(projection, query)?.size).toBe(1);
  for (const query of ['background', 'runtime metadata']) expect(searchItems(projection, query)?.size).toBe(0);
  expect(modelToolArgumentPreview(undefined)).toBeUndefined();
  expect(modelToolArgumentPreview(null)).toBeUndefined();
});
