// Browser presentation contract; native Artifact ownership is covered by Rust
// artifact/read regressions. This fixture uses the production resource owner.
import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Artifact, ArtifactContext } from '../../src/app/components/Artifact';
import { ArtifactPreview, PreviewContext, type PreviewArtifact } from '../../src/app/components/ArtifactPreview';
import { FilePreviewResources } from '../../src/client/session-files';
import { WorkspaceAuthority } from '../../src/workspaces/authority';
import { Server } from '../fixture';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/reset.css';
const server = new Server();
const original = '# Managed report\r\n\r\nOriginal **bytes**: 报告\r\n';
server.handlers.set('artifact/read', () => ({ type: 'artifact_bytes', data: btoa(String.fromCharCode(...new TextEncoder().encode(original))) }));
await server.attached('A');
const resources = new FilePreviewResources(server.client, 'A', server.workspaceHost, new WorkspaceAuthority(server.workspaceHost));
function Fixture() {
  const [file, select] = useState<PreviewArtifact>();
  return <PreviewContext.Provider value={select}><ArtifactContext.Provider value={resources.artifacts}>
    {(['md', 'txt', 'rs'] as const).map(extension => <Artifact key={extension} id={`artifact-${extension}`} name={`报告 managed file.${extension}`} mimeType={extension === 'md' ? 'text/markdown' : 'text/plain'}/>)}
    <button onClick={() => select(undefined)}>Close preview</button>
    {file && <ArtifactPreview key={JSON.stringify(file.source)} artifact={file} resources={resources}/>}
  </ArtifactContext.Provider></PreviewContext.Provider>;
}
Object.assign(window, { managedPreviewRequests: server.requests });
createRoot(document.getElementById('root')!).render(<Fixture/>);
