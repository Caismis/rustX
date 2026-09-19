import { describe, it, expect, vi } from 'vitest';
import { SessionExportController, downloadArchive } from '../src/client/session-export';
import { archiveDownloadUrl } from '../../protocol/app-server/download';

const download = { path: `/session-archive/${'a'.repeat(43)}`, filename: 'rustx-session-A.zip', expires_in_seconds: 60, loopback_port: null };
describe('native Session export consumer', () => {
  it('coalesces rapid gestures into one native preflight and browser save', async () => {
    let release!: () => void;
    const gate = new Promise<void>(resolve => { release = resolve; });
    const prepare = vi.fn(async () => { await gate; return { download, endpoint: 'wss://native.example/' }; });
    const save = vi.fn();
    const controller = new SessionExportController(prepare, save);
    const first = controller.download('A');
    expect(controller.download('A')).toBe(first);
    await Promise.resolve(); expect(prepare).toHaveBeenCalledOnce(); expect(save).not.toHaveBeenCalled();
    release(); await first;
    expect(save).toHaveBeenCalledWith(`https://native.example${download.path}`, download.filename);
  });
  it('preserves native/authentication failures and never starts a download', async () => {
    const save = vi.fn();
    const controller = new SessionExportController(async () => { throw new Error('Unauthorized'); }, save);
    await expect(controller.download('A')).rejects.toThrow('Unauthorized');
    expect(save).not.toHaveBeenCalled();
  });
  it('uses a browser anchor without reading or composing bytes', () => {
    const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (this: HTMLAnchorElement) {
      expect(this.href).toBe(`https://native.example${download.path}`);
      expect(this.download).toBe(download.filename);
      expect(this.referrerPolicy).toBe('no-referrer');
    });
    downloadArchive(`https://native.example${download.path}`, download.filename);
    expect(click).toHaveBeenCalledOnce(); click.mockRestore();
  });
  it('refuses authority substitution or a remote-selected local port', () => {
    expect(() => archiveDownloadUrl({ ...download, path: 'https://evil.example/' }, 'wss://native.example/')).toThrow();
    expect(() => archiveDownloadUrl({ ...download, loopback_port: 1234 }, 'wss://native.example/')).toThrow();
  });
});

describe('generated native preparation failures through the Web client', () => {
  it('keeps descendant/artifact diagnostics, coalesces failures and never starts a download', async () => {
    const { fixtures } = await import('../../protocol/app-server/fixtures');
    const { Server } = await import('./fixture');
    const server = new Server();
    await server.connect();
    server.held.add('session/exportPrepare');
    const save = vi.fn();
    const controller = new SessionExportController(async id => {
      const { download } = await server.client.request({ method: 'session/exportPrepare', params: { session_id: id } }, 'session_archive');
      return { download, endpoint: 'ws://remote.example/' };
    }, save);
    let count = 0;
    try {
      for (const fixture of fixtures) {
        const failure = 'error' in fixture ? fixture.error : undefined;
      if (!failure || failure.data?.kind !== 'archive_preparation_failed') continue;
        const pending = controller.download('session-A');
        expect(controller.download('session-A')).toBe(pending);
        const rejected = expect(pending).rejects.toThrow(failure.message);
        await Promise.resolve();
        const requests = server.requests.filter(item => item.request.method === 'session/exportPrepare');
        expect(requests).toHaveLength(++count);
        const request = requests.at(-1)!.request;
        expect(request.params).toEqual({ session_id: 'session-A' });
        server.socket.deliver({ jsonrpc: '2.0', id: request.id, error: failure });
        await rejected;
        expect(save).not.toHaveBeenCalled();
      }
      expect(count).toBe(2);
    } finally { server.client.disconnect(); }
  });
});
