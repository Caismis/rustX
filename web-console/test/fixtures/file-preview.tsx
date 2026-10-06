// Browser presentation contract; native Artifact ownership is covered by Rust
// artifact/read regressions. This fixture uses the production resource owner.
import { useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Artifact, ArtifactContext } from '../../src/app/components/Artifact';
import { ArtifactPreview, PreviewContext, type PreviewArtifact } from '../../src/app/components/ArtifactPreview';
import { FilePreviewCoordinator, type FilePreviewLease } from '../../src/client/session-files';
import { ArtifactResources } from '../../src/client/artifacts';
import type { PreviewViewState } from '../../src/presentation/right-panel/preview-view-state';
import { WorkspaceAuthority } from '../../src/workspaces/authority';
import { Server } from '../fixture';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/reset.css';
const server = new Server();
const original = '# Managed report\r\n\r\nOriginal **bytes**: 报告\r\n';
server.handlers.set('artifact/read', () => ({ type: 'artifact_bytes', data: btoa(String.fromCharCode(...new TextEncoder().encode(original))) }));
await server.attached('A');
const resources = new FilePreviewCoordinator(server.client, 'A', server.workspaceHost, new WorkspaceAuthority(server.workspaceHost));
const inline = new ArtifactResources(server.client, 'A');
function Fixture() {
  const [file, setFile] = useState<{ artifact: PreviewArtifact; resources: FilePreviewLease; occurrence: number }>();
  const [viewState, setViewState] = useState<PreviewViewState>({}), occurrence = useRef(0), active = useRef<FilePreviewLease>(undefined);
  const select = (artifact?: PreviewArtifact) => {
    active.current?.dispose(); setViewState({});
    active.current = artifact ? resources.acquire(++occurrence.current, artifact.source) : undefined;
    setFile(artifact ? { artifact, resources: active.current!, occurrence: occurrence.current } : undefined);
  };
  const download = (artifact: PreviewArtifact) => { void resources.download(artifact.source, artifact.name, artifact.mimeType); };
  return <PreviewContext.Provider value={{ openPreview: select, download }}><ArtifactContext.Provider value={inline}>
    {(['md', 'txt', 'rs'] as const).map(extension => <Artifact key={extension} id={`artifact-${extension}`} name={`报告 managed file.${extension}`} mimeType={extension === 'md' ? 'text/markdown' : 'text/plain'}/>)}
    <button onClick={() => select(undefined)}>Close preview</button>
    {file && <ArtifactPreview key={file.occurrence} artifact={file.artifact} resources={file.resources} viewState={viewState} onViewStateChange={patch => setViewState(state => ({ ...state, ...patch }))} onDownload={() => download(file.artifact)}/>}
  </ArtifactContext.Provider></PreviewContext.Provider>;
}
Object.assign(window, { managedPreviewRequests: server.requests });
createRoot(document.getElementById('root')!).render(<Fixture/>);
