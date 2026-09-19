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
