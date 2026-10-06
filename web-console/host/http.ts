import type { IncomingMessage, ServerResponse } from 'node:http';
import { WorkspaceHostError, type ProductHostWorkspaces } from '../src/workspaces/host.ts';
import { AppServerRequestError, UncertainOutcomeError } from '../../tui/src/app-server/client.ts';
import { NativeFileReadError } from './file-read.ts';
/** Workspace authority only. The launcher carrier authenticates before this handler;
 * independently managed deployments supply their own browser authentication. */
export function workspaceHandler(host?: ProductHostWorkspaces) {
  // One active document carrier. The token cancels this operation only; it is
  // neither a tab registry nor native authority, and is forgotten at settlement.
  type FileOperation = { id: string; scope: { authorityId: string; endpoint: string }; abort: AbortController };
  let document: FileOperation | undefined;
  const files = new Map<string, FileOperation>();
  return async (request: IncomingMessage, response: ServerResponse, next: () => void = () => { response.writeHead(404).end(); }) => {
    if (!request.url?.startsWith('/product-host/')) return next();
    try {
      if (!host) throw new Error('No Product Host configured');
      if (request.method !== 'POST' || request.headers['content-type'] !== 'application/json') throw new Error('Expected JSON POST');
      if (request.headers.origin && new URL(request.headers.origin).host !== request.headers.host) throw new Error('Cross-origin Host request refused');
      const chunks: Buffer[] = []; let size = 0;
      for await (const chunk of request) {
        const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
        size += bytes.byteLength; if (size > 65536) throw new Error('Host request too large');
        chunks.push(bytes);
      }
      const body = JSON.parse(Buffer.concat(chunks).toString('utf8'));
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
        case 'desktop-catalog':
          if (!host.desktopCatalog) throw new Error('Desktop unavailable on this Product Host');
          if (Object.keys(body).some(key => !['scope', 'refresh'].includes(key)) || typeof body.refresh !== 'boolean') throw new Error('Invalid desktop request');
          value = await host.desktopCatalog(scope(), body.refresh); break;
        case 'desktop-open':
          if (!host.openWorkspace) throw new Error('Desktop unavailable on this Product Host');
          if (Object.keys(body).some(key => !['scope', 'target', 'application'].includes(key))) throw new Error('Invalid desktop request');
          value = await host.openWorkspace(scope(), body.target, body.application); break;
        case 'document-preview':
          if (!host.previewDocument) throw new Error('preview_unavailable');
          {
            if (Object.keys(body).some(key => !['scope', 'request', 'operationId'].includes(key))
              || typeof body.operationId !== 'string' || !/^[a-f0-9-]{36}$/.test(body.operationId)) throw new Error('Invalid document operation');
            if (document) throw new Error('capacity');
            const owned = { id: body.operationId as string, scope: scope(), abort: new AbortController() };
            document = owned;
            const operation = owned.abort;
            const closed = () => { if (!response.writableFinished) operation.abort(); };
            response.on('close', closed);
            // Header acknowledgement precedes cancellation admission. The browser
            // keeps this response open until the Host's physical finally settles.
            response.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store', 'X-Rustx-Document-Operation': owned.id });
            response.flushHeaders();
            try {
              const result = await host.previewDocument(owned.scope, body.request, operation.signal);
              response.end(JSON.stringify({ ok: true, value: result }));
            } catch (error) {
              response.end(JSON.stringify({ ok: false, message: String(error), kind: error instanceof WorkspaceHostError ? error.kind : undefined }));
            } finally { response.off('close', closed); if (document === owned) document = undefined; }
            return;
          }
        case 'document-cancel': {
          if (Object.keys(body).some(key => !['scope', 'operationId'].includes(key)) || typeof body.operationId !== 'string') throw new Error('Invalid document cancellation');
          const captured = scope();
          if (document && document.id === body.operationId && document.scope.authorityId === captured.authorityId && document.scope.endpoint === captured.endpoint) document.abort.abort();
          break;
        }
        case 'file-read':
          if (!host.readDelivery) throw new Error('Session file reads unavailable on this Product Host');
          {
            if (Object.keys(body).some(key => !['scope', 'read', 'operationId'].includes(key))
              || typeof body.operationId !== 'string' || !/^[a-f0-9-]{36}$/.test(body.operationId)) throw new Error('Invalid file operation');
            if (files.size >= 2 || files.has(body.operationId)) throw new Error('capacity');
            const owned: FileOperation = { id: body.operationId, scope: scope(), abort: new AbortController() };
            files.set(owned.id, owned);
            const closed = () => { if (!response.writableFinished) owned.abort.abort(); };
            response.on('close', closed);
            response.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store', 'X-Rustx-File-Operation': owned.id });
            response.flushHeaders();
            try {
              const result = await host.readDelivery(owned.scope, body.read, owned.abort.signal);
              response.end(JSON.stringify({ ok: true, value: result }));
            } catch (error) {
              response.end(JSON.stringify({ ok: false, message: String(error), kind: error instanceof WorkspaceHostError ? error.kind : undefined,
                nativeError: error instanceof AppServerRequestError || error instanceof NativeFileReadError ? error.error : undefined }));
            } finally { response.off('close', closed); files.delete(owned.id); }
            return;
          }
        case 'file-cancel': {
          if (Object.keys(body).some(key => !['scope', 'operationId'].includes(key)) || typeof body.operationId !== 'string') throw new Error('Invalid file cancellation');
          const captured = scope(), owned = files.get(body.operationId);
          if (owned && owned.scope.authorityId === captured.authorityId && owned.scope.endpoint === captured.endpoint) owned.abort.abort();
          break;
        }
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
