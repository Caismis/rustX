/** A focused real-native seam, not a manufactured App Server or Host response. */
import { useRef, useState, useSyncExternalStore } from 'react';
import { createRoot } from 'react-dom/client';
import { AppServerClient } from '../../src/client/app-server';
import { FilePreviewCoordinator, type FilePreviewLease } from '../../src/client/session-files';
import { HttpWorkspaceHost } from '../../src/workspaces/http-host';
import { WorkspaceSessionNavigation } from '../../src/workspaces/navigation';
import { WorkspaceAuthority } from '../../src/workspaces/authority';
import { ArtifactPreview, type PreviewArtifact } from '../../src/app/components/ArtifactPreview';
import type { PreviewViewState } from '../../src/presentation/right-panel/preview-view-state';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/reset.css';
const client = new AppServerClient(), host = new HttpWorkspaceHost(), authority = new WorkspaceAuthority(host);
client.setAttachmentAdmission(new WorkspaceSessionNavigation(authority, client, client.navigation).admit);
Object.assign(window, { artifactDocument: {
  async setup(endpoint: string, token: string, session: string, id: string) {
    await client.connect(endpoint, token); await client.attach(session); await authority.observe();
    const resources = new FilePreviewCoordinator(client, session, host, authority);
    function View() {
      useSyncExternalStore(client.subscribe, client.getSnapshot);
      const [selected, set] = useState<{ artifact: PreviewArtifact; lease: FilePreviewLease }>();
      const [viewState, setViewState] = useState<PreviewViewState>({}), occurrence = useRef(0);
      const open = () => {
        selected?.lease.dispose(); setViewState({});
        const artifact: PreviewArtifact = { source: { kind: 'artifact', id }, name: '报告 original.xlsx', image: false };
        set({ artifact, lease: resources.acquire(++occurrence.current, artifact.source) });
      };
      return <><button onClick={open}>Open Artifact</button>
        <button onClick={() => { selected?.lease.dispose(); set(undefined); }}>Close preview</button>
        {!resources.current() ? <p role="alert">Preview authority changed</p> : selected && <ArtifactPreview key={selected.lease.occurrenceId} artifact={selected.artifact} resources={selected.lease} viewState={viewState} onViewStateChange={patch => setViewState(state => ({ ...state, ...patch }))} onDownload={() => { void resources.download(selected.artifact.source, selected.artifact.name); }}/>}</>;
    }
    createRoot(document.getElementById('root')!).render(<View/>);
    Object.assign((window as any).artifactDocument, {
      target: () => client.target(session),
      replaceAttachment: async () => { resources.dispose(); await client.release(session); await client.attach(session); },
      dispose: async () => { resources.dispose(); await client.dispose(); },
    });
  },
} });
