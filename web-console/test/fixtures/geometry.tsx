/** Actual delivery/workspace presentation and source owners; deterministic source bytes. */
import { useState, useSyncExternalStore } from 'react';
import { createRoot } from 'react-dom/client';
import { DeliveryCards } from '../../src/app/components/Artifact';
import { PreviewContext } from '../../src/app/components/ArtifactPreview';
import { PreviewWorkspace } from '../../src/app/components/PreviewWorkspace';
import { PreviewWorkspaceOwner } from '../../src/app/preview-workspace';
import { WorkspaceFile, fileTabView, type WorkbenchCall } from '../../src/app/components/WorkbenchFiles';
import { WorkspaceAuthority } from '../../src/workspaces/authority';
import { WorkspaceHostError, type ProductHostWorkspaces } from '../../src/workspaces/host';
import { localeController } from '../../src/locale/controller';
import { applyTheme } from '../../src/app/appearance';
import { Server } from '../fixture';
import type { SessionFileReference } from '../../../protocol/app-server/v62';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/reset.css';

const names = ['cube.glb', 'cube.stl', 'cube-ascii.stl', 'cube.step', 'cube-inch.step', 'cube.iges', 'cube.brep', 'cube-textured.glb'];
const files = new Map<string, { data: Uint8Array<ArrayBuffer>; version: number }>();
const hints = new Set<() => void>(), reads: { name: string; version: number; probe: boolean }[] = [];
let available = true, hostId = 'geometry-host';
function file(name: string): SessionFileReference { return { scope: { conversation_id: 'original', device: '1', inode: String(names.indexOf(name) + 1) }, path: name, name, mime_type: 'application/octet-stream' }; }
async function source(name: string, probe: boolean) {
  if (!available) throw new WorkspaceHostError('revoked', 'authorization_revoked');
  if (!files.has(name)) { const response = await fetch(`/test/fixtures/geometry/${name}`); if (!response.ok) throw new Error('Fixture missing'); files.set(name, { data: new Uint8Array(await response.arrayBuffer()), version: 1 }); }
  const value = files.get(name)!; reads.push({ name, version: value.version, probe });
  return { data: probe ? value.data.slice(0, 1) : value.data.slice(), version: String(value.version), bytes: value.data.length, offset: 0, eof: !probe || value.data.length <= 1 };
}
function observe(changed: () => void) {
  hints.add(changed); let resolve!: () => void; const done = new Promise<void>(finish => { resolve = finish; });
  return { version: '1', done, close: async () => { hints.delete(changed); resolve(); } };
}
const server = new Server(); await server.attached('A', 'B');
const host: ProductHostWorkspaces = { ...server.workspaceHost,
  listWorkspaces: async () => ({ ...await server.workspaceHost.listWorkspaces(), authorityId: hostId }),
  readDelivery: async (_scope, read) => {
    if (read.locator.type !== 'by_message_id') throw new Error('Wrong fixture source');
    const name = read.locator.message_id.replace('geometry:', '');
    if (read.read?.kind !== 'bytes') throw new Error('Geometry must use byte reads');
    return { file: file(name), ...await source(name, !!read.read.range) };
  },
  observeDelivery: async (_scope, _read, _file, changed) => observe(changed),
};
const authority = new WorkspaceAuthority(host); await authority.observe();
const owner = new PreviewWorkspaceOwner(server.client, host, authority); owner.start(); owner.selectSession('A');
const workspaceViews = new Map(names.map(name => [name, fileTabView(undefined, () => available)]));
const call: WorkbenchCall = Object.assign(async (request: Parameters<WorkbenchCall>[0]) => {
  if (request.kind === 'preview') return { cwd: '/workspace', preview: await source(request.path, request.read.kind === 'bytes' && !!request.read.range) };
  return {};
}, { observe: async (_path: string, changed: () => void) => observe(changed) });

function Fixture() {
  const snapshot = useSyncExternalStore(owner.subscribe, owner.getSnapshot);
  const [workbench, setWorkbench] = useState<string>(), [showWorkbench, setShowWorkbench] = useState(true);
  return <PreviewContext.Provider value={{ openPreview: artifact => { setWorkbench(undefined); owner.openPreview(artifact); }, download: artifact => { void owner.download(artifact); } }}>
    <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
      <button onClick={() => owner.collapse()}>Hide delivery</button><button onClick={() => owner.reveal()}>Show delivery</button>
      <button onClick={() => setShowWorkbench(value => !value)}>Toggle workspace body</button>
      <button onClick={() => { available = false; hostId = 'retired-host'; void authority.observe(); for (const view of workspaceViews.values()) view.preview?.dispose(); setWorkbench(undefined); }}>Revoke authority</button>
      <button onClick={() => localeController.setLocale('zh')}>Chinese</button>
      <button onClick={() => applyTheme('dark')}>Dark theme</button>
    </div>
    <div style={{ display: 'flex', gap: 4, flexWrap: 'wrap' }}>{names.map(name => <button key={name} onClick={() => { owner.showWorkbench(); setWorkbench(name); setShowWorkbench(true); }}>Workspace {name}</button>)}</div>
    <DeliveryCards conversationId="conversation-A" deliveries={names.map(name => ({ key: name, locator: { type: 'by_message_id', message_id: `geometry:${name}` }, index: 0, file: file(name) }))}/>
    <div style={{ height: 760, width: '100%' }}>
      <PreviewWorkspace owner={owner} snapshot={snapshot} focusRequest={0} returnFocus={() => {}}/>
      {workbench && showWorkbench && <WorkspaceFile key={workbench} path={workbench} call={call} view={workspaceViews.get(workbench)!} onOpen={() => {}}/>}
    </div>
  </PreviewContext.Provider>;
}
Object.assign(window, { geometryContract: {
  reads, source: (name: string) => [...files.get(name)!.data],
  replace(name: string, data: number[]) { files.set(name, { data: new Uint8Array(data), version: (files.get(name)?.version ?? 0) + 1 }); hints.forEach(changed => changed()); },
  grid(side: number) {
    if (!Number.isSafeInteger(side) || side < 1 || side > 350) throw new Error('Invalid benchmark size');
    const data = new Uint8Array(84 + side * side * 2 * 50), view = new DataView(data.buffer); view.setUint32(80, side * side * 2, true);
    let at = 84;
    for (let y = 0; y < side; y++) for (let x = 0; x < side; x++) for (const points of [[x,y,0, x+1,y,0, x,y+1,0], [x+1,y,0, x+1,y+1,0, x,y+1,0]]) {
      view.setFloat32(at + 8, 1, true); points.forEach((value, index) => view.setFloat32(at + 12 + index * 4, value, true)); at += 50;
    }
    files.set('cube.stl', { data, version: 1 });
  },
  mutate() {
    const value = files.get('cube.stl')!; const next = value.data.slice(), data = new DataView(next.buffer);
    for (let triangle = 0; triangle < data.getUint32(80, true); triangle++) for (let coordinate = 0; coordinate < 9; coordinate++) {
      const at = 84 + triangle * 50 + 12 + coordinate * 4; data.setFloat32(at, data.getFloat32(at, true) * 2, true);
    }
    files.set('cube.stl', { data: next, version: value.version + 1 }); hints.forEach(changed => changed());
  },
  snapshot: () => owner.getSnapshot().workspace?.tabs.map(tab => ({ name: tab.artifact.name, state: tab.view.geometry })),
} });
createRoot(document.getElementById('root')!).render(<Fixture/>);
