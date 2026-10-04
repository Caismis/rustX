/** A focused real-native seam, not a manufactured App Server or Host response. */
import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { AppServerClient } from '../../src/client/app-server';
import { FilePreviewResources } from '../../src/client/session-files';
import { HttpWorkspaceHost } from '../../src/workspaces/http-host';
import { WorkspaceSessionNavigation } from '../../src/workspaces/navigation';
import { WorkspaceAuthority } from '../../src/workspaces/authority';
import { ArtifactPreview, type PreviewArtifact } from '../../src/app/components/ArtifactPreview';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/reset.css';
const client = new AppServerClient(), host = new HttpWorkspaceHost(), authority = new WorkspaceAuthority(host);
client.setAttachmentAdmission(new WorkspaceSessionNavigation(authority, client, client.navigation).admit);
Object.assign(window, { artifactDocument: {
  async setup(endpoint: string, token: string, session: string, id: string) {
    await client.connect(endpoint, token); await client.attach(session); await authority.observe();
    const resources = new FilePreviewResources(client, session, host, authority);
    function View() {
      const [artifact, set] = useState<PreviewArtifact>();
      return <><button onClick={() => set({ source: { kind: 'artifact', id }, name: '报告 original.xlsx', image: false })}>Open Artifact</button>
        <button onClick={() => set(undefined)}>Close preview</button>
        {artifact && <ArtifactPreview artifact={artifact} resources={resources}/>}</>;
    }
    createRoot(document.getElementById('root')!).render(<View/>);
    Object.assign((window as any).artifactDocument, {
      target: () => client.target(session),
      replaceAttachment: async () => { await client.release(session); await client.attach(session); },
      dispose: async () => { resources.dispose(); await client.dispose(); },
    });
  },
} });
