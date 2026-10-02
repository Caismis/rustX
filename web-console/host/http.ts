import type { IncomingMessage, ServerResponse } from 'node:http';
import { WorkspaceHostError, type ProductHostWorkspaces } from '../src/workspaces/host.ts';
import { AppServerRequestError, UncertainOutcomeError } from '../../tui/src/app-server/client.ts';
import { NativeFileReadError } from './file-read.ts';
/** Workspace authority only. The launcher carrier authenticates before this handler;
 * independently managed deployments supply their own browser authentication. */
export function workspaceHandler(host?: ProductHostWorkspaces) {
  return async (request: IncomingMessage, response: ServerResponse, next: () => void = () => { response.writeHead(404).end(); }) => {
    if (!request.url?.startsWith('/product-host/')) return next();
    try {
      if (!host) throw new Error('No Product Host configured');
      if (request.method !== 'POST' || request.headers['content-type'] !== 'application/json') throw new Error('Expected JSON POST');
      if (request.headers.origin && new URL(request.headers.origin).host !== request.headers.host) throw new Error('Cross-origin Host request refused');
      let text = '';
      for await (const chunk of request) { text += String(chunk); if (text.length > 65536) throw new Error('Host request too large'); }
      const body = JSON.parse(text);
      if (!body || typeof body !== 'object' || Array.isArray(body)) throw new Error('Expected Host request object');
      const string = (key: string) => { if (typeof body[key] !== 'string') throw new Error(`Expected ${key}`); return body[key] as string; };
      const scope = () => {
        const value = body.scope;
        if (!value || typeof value !== 'object' || Array.isArray(value)
          || typeof value.authorityId !== 'string' || !value.authorityId
          || typeof value.endpoint !== 'string' || !URL.canParse(value.endpoint)) throw new Error('Expected Product Host scope');
        return { authorityId: value.authorityId as string, endpoint: value.endpoint as string };
      };
      let value: unknown;
      switch (request.url.slice('/product-host/'.length)) {
        case 'file-read':
          if (!host.readDelivery) throw new Error('Session file reads unavailable on this Product Host');
          {
            const read = new AbortController();
            const closed = () => { if (!response.writableFinished) read.abort(); };
            response.on('close', closed);
            try { value = await host.readDelivery(scope(), body.read, read.signal); }
            finally { response.off('close', closed); }
          }
          break;
        case 'list': value = await host.listWorkspaces(); break;
        case 'adopt': value = await host.adoptWorkspace(scope(), string('location')); break;
        case 'rename': value = await host.renameWorkspace(scope(), string('id'), string('displayName')); break;
        case 'reorder': value = await host.reorderWorkspace(scope(), string('id'), body.before === undefined ? undefined : string('before')); break;
        case 'remove': value = await host.removeWorkspace(scope(), string('id')); break;
        case 'resolve': value = await host.resolveWorkspace(string('id'), string('endpoint')); break;
        case 'configuration':
          if (!host.configureWorkspace) throw new Error('Workspace configuration is unavailable on this Host');
          value = await host.configureWorkspace(string('id'), string('endpoint'), body.operation); break;
        case 'classify': {
          if (!Array.isArray(body.cwds) || body.cwds.some((cwd: unknown) => typeof cwd !== 'string')) throw new Error('Expected bounded cwds');
          value = await host.classifyLocations(body.cwds, string('endpoint'), body.authorityId === undefined ? undefined : string('authorityId')); break;
        }
        default: throw new Error('Unknown Host operation');
      }
      response.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' }).end(JSON.stringify(value ?? null));
    } catch (error) {
      response.writeHead(400, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' }).end(JSON.stringify({
        message: String(error),
        kind: error instanceof WorkspaceHostError ? error.kind : undefined,
        nativeError: error instanceof AppServerRequestError || error instanceof NativeFileReadError ? error.error : undefined,
        uncertain: error instanceof UncertainOutcomeError,
      }));
    }
  };
}
