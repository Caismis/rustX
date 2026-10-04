import { afterEach, expect, it, vi } from 'vitest';
import { RequestNotDispatched } from '../src/client/app-server';
import { AttachmentIntake, UploadFailure, type UploadPort } from '../src/client/uploads';
import { Server, capabilities } from './fixture';

const servers: Server[] = [];
afterEach(() => { for (const server of servers.splice(0)) server.client.disconnect(); });
async function connected() {
  const server = new Server(); servers.push(server); await server.attached('A'); return server;
}
function reads(server: Server, count: number) {
  server.held.add('session/settings');
  return Array.from({ length: count }, () => server.client.request(
    { method: 'session/settings', params: { session_id: 'A' } }, 'settings',
  ).catch(error => error));
}
function count(server: Server, method: 'session/uploadPrepare' | 'session/uploadStatus') {
  return server.requests.filter(row => row.request.method === method).length;
}

it('unsent queued prepare is known failed, with no read or carrier; explicit intake Retry sends only a fresh identity', async () => {
  const server = await connected();
  const held = reads(server, 8);
  const owner = new AttachmentIntake();
  owner.add([{ file: new File(['document'], 'document.txt') }], capabilities.upload_policy);
  const initial = owner.snapshot()[0];
  let failure: unknown;
  const port: UploadPort = {
    upload: (files, operation) => server.client.upload('A', [...files], undefined, operation).catch(error => { failure = error; throw error; }),
    status: vi.fn(operation => server.client.uploadStatus('A', operation)),
  };
  const work = owner.upload(initial.id, port);
  expect(count(server, 'session/uploadPrepare')).toBe(0);
  // Eight sent reads + the unsent prepare + 55 queued reads fill all 64
  // registrations. This proves prepare actually owns a pending slot.
  const queued = reads(server, 55);
  await expect(server.client.request({ method: 'session/settings', params: { session_id: 'A' } }, 'settings')).rejects.toBeInstanceOf(RequestNotDispatched);
  server.socket.close();
  await Promise.all([work, ...held, ...queued]);
  expect(failure).toBeInstanceOf(UploadFailure);
  expect(failure).toMatchObject({ state: 'failed', cause: expect.any(RequestNotDispatched) });
  expect(owner.snapshot()[0]).toMatchObject({ status: 'failed', operation: initial.operation });
  expect(count(server, 'session/uploadPrepare')).toBe(0);
  expect(count(server, 'session/uploadStatus')).toBe(0);
  expect(server.carrierTransfers).toBe(0);
  await owner.reconcile(initial.id, port);
  expect(port.status).not.toHaveBeenCalled();

  server.held.delete('session/settings');
  await server.connect();
  server.held.add('session/uploadPrepare');
  const retry = owner.upload(initial.id, port);
  await owner.upload(initial.id, port); // duplicate Retry owns no second action
  const request = await server.waitFor('session/uploadPrepare', 1);
  expect(request.params).toMatchObject({ operation_id: owner.snapshot()[0].operation });
  expect(owner.snapshot()[0].operation).not.toBe(initial.operation);
  server.reply(request); await retry;
  expect(owner.snapshot()[0].status).toBe('ready');
  expect(count(server, 'session/uploadPrepare')).toBe(1);
  expect(count(server, 'session/uploadStatus')).toBe(1);
  expect(server.carrierTransfers).toBe(1);
});

it('immediate request capacity refusal is a known upload failure without native reads or carrier', async () => {
  const server = await connected(); const held = reads(server, 64);
  await expect(server.client.upload('A', [new File(['x'], 'x')])).rejects.toMatchObject({
    state: 'failed', cause: expect.any(RequestNotDispatched),
  });
  expect(count(server, 'session/uploadPrepare')).toBe(0);
  expect(count(server, 'session/uploadStatus')).toBe(0);
  expect(server.carrierTransfers).toBe(0);
  server.socket.close(); await Promise.all(held);
});

it.each(['close', 'send throws'] as const)('prepare dispatch followed by %s remains uncertain and is never replayed', async failure => {
  const server = await connected(); server.held.add('session/uploadPrepare');
  if (failure === 'send throws') {
    const send = server.socket.send.bind(server.socket);
    vi.spyOn(server.socket, 'send').mockImplementation(raw => { send(raw); throw new Error('transport failure'); });
  }
  const operation = 'f'.repeat(32);
  const work = server.client.upload('A', [new File(['x'], 'x')], undefined, operation);
  const rejected = expect(work).rejects.toMatchObject({ state: 'uncertain' });
  await server.waitFor('session/uploadPrepare', 1);
  if (failure === 'close') server.socket.close();
  await rejected;
  expect(server.client.getSnapshot().uncertain).toMatchObject([{ uploadOperationId: operation }]);
  expect(count(server, 'session/uploadStatus')).toBe(0);
  expect(server.carrierTransfers).toBe(0);
  await server.connect();
  expect(count(server, 'session/uploadPrepare')).toBe(1);
  expect(count(server, 'session/uploadStatus')).toBe(0);
});

it.each(['current false', 'current throws', 'validation false', 'validation rejects', 'validation throws', 'validation timeout'] as const)(
  '%s refuses before dispatch with structural certainty and releases its reservation',
  async mode => {
    const server = await connected();
    vi.useFakeTimers();
    try {
      const proof = {
        current: () => {
          if (mode === 'current throws') throw new Error('local authority failure');
          return mode !== 'current false';
        },
        validate: () => {
          if (mode === 'validation throws') throw new Error('local validation failure');
          if (mode === 'validation rejects') return Promise.reject(new Error('read failed'));
          if (mode === 'validation timeout') return new Promise<boolean>(() => {});
          return Promise.resolve(false);
        },
      };
      const work = server.client.request({
        method: 'session/uploadPrepare',
        params: { target: server.client.target('A'), operation_id: 'a'.repeat(32), files: [{ name: 'a', size: 1 }] },
      }, 'upload_prepared', undefined, proof);
      const rejected = expect(work).rejects.toBeInstanceOf(RequestNotDispatched);
      if (mode === 'validation timeout') await vi.advanceTimersByTimeAsync(30_000);
      await rejected;
      expect(count(server, 'session/uploadPrepare')).toBe(0);
      expect(server.client.getSnapshot().uncertain).toEqual([]);
      // No leaked reservation prevents subsequent ordinary requests.
      await server.client.request({ method: 'session/settings', params: { session_id: 'A' } }, 'settings');
    } finally { vi.useRealTimers(); }
  },
);
