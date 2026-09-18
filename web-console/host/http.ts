import type { IncomingMessage, ServerResponse } from 'node:http';
import type { ProductHostWorkspaces } from '../src/workspaces/host.ts';
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
      const string = (key: string) => { if (typeof body[key] !== 'string') throw new Error(`Expected ${key}`); return body[key] as string; };
      let value: unknown;
      switch (request.url.slice('/product-host/'.length)) {
        case 'list': value = await host.listWorkspaces(); break;
        case 'adopt': value = await host.adoptWorkspace(string('location')); break;
        case 'rename': value = await host.renameWorkspace(string('id'), string('displayName')); break;
        case 'reorder': value = await host.reorderWorkspace(string('id'), body.before === undefined ? undefined : string('before')); break;
        case 'remove': value = await host.removeWorkspace(string('id')); break;
        case 'resolve': value = await host.resolveWorkspace(string('id'), string('endpoint')); break;
        case 'classify': {
          if (!Array.isArray(body.cwds) || body.cwds.some((cwd: unknown) => typeof cwd !== 'string')) throw new Error('Expected bounded cwds');
          value = await host.classifyLocations(body.cwds, string('endpoint')); break;
        }
        default: throw new Error('Unknown Host operation');
      }
      response.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' }).end(JSON.stringify(value ?? null));
    } catch (error) { response.writeHead(400, { 'Content-Type': 'text/plain', 'Cache-Control': 'no-store' }).end(String(error)); }
  };
}
