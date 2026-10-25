import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { AppServerClient } from '../../src/client/app-server';
import { ComposerDraft } from '../../src/app/composer/draft';
import { AttachmentIntake } from '../../src/client/uploads';
import { CommandSession } from '../../src/app/commands/native';
import type { Request } from '../../../protocol/app-server/v57';

function barrier<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
for (const replacement of ['reattach', 'switch'] as const) test(`real native ${replacement} settles old input by canonical Conversation, not attachment`, async () => {
  const fixture = await startDogfood('web_composer_identity');
  const requests: Request[] = [], methods = new Map<string, string>(), received = barrier<() => void>();
  let hold = false, socket!: WebSocket, sessionId: string | undefined, passed = false;
  const client = new AppServerClient((url, protocols) => {
    socket = new WebSocket(url, protocols);
    socket.addEventListener('message', event => {
      const reply = JSON.parse(String(event.data));
      if (hold && methods.get(String(reply.id)) === 'turn/start') {
        event.stopImmediatePropagation();
        received.resolve(() => socket.onmessage?.(event));
      }
    });
    const send = socket.send.bind(socket);
    socket.send = data => { const request = JSON.parse(String(data)) as Request; requests.push(request); methods.set(String(request.id), request.method); send(data); };
    return socket;
  });
  // This authenticated test peer created the Session in its own fixture root.
  // Its lifecycle admission is restricted to that exact native Session.
  client.setSessionAdmission(async (id, current) => id === sessionId ? { current, validate: async () => current() } : false);
  const terminal = (previous?: string) => {
    const done = barrier<void>();
    const release = client.subscribe(() => {
      const attempt = sessionId ? client.getSnapshot().views[sessionId]?.snapshot?.attempt : undefined;
      if (attempt && attempt.attempt_id !== previous && attempt.phase.type === 'settled') { release(); done.resolve(); }
    });
    return done.promise;
  };
  try {
    await client.connect(fixture.endpoint, fixture.token);
    const created = await client.request({ method: 'session/create', params: { settings: { cwd: fixture.workspaceA } } }, 'session_transition');
    sessionId = created.session.id;
    await client.request({ method: 'session/name', params: { session_id: sessionId, name: 'New session' } }, 'session'); await client.attach(sessionId, created.session.active_node);
    const firstDone = terminal(); await client.send(sessionId, 'Composer identity 0'); await firstDone;
    const target = client.target(sessionId);
    let branch: typeof created | undefined;
    if (replacement === 'switch') {
      const cut = await client.request({ method: 'session/boundaries', params: { target, offset: 0, limit: 32 } }, 'boundaries');
      branch = await client.request({ method: 'session/branch', params: { session_id: sessionId, node_id: created.session.active_node, surface_revision: cut.surface_revision, boundary: cut.boundaries[0].locator, side: 'before' } }, 'session_transition');
    }
    const owner = new ComposerDraft(), intake = new AttachmentIntake();
    const editor = owner.activate('same-editor', [], 'old-proof', target);
    owner.edit(editor.incarnation, 'Composer identity 1'); const ticket = owner.submit(editor.incarnation, intake, 'send', target)!;
    hold = true;
    // Avoid observing the previous settled Attempt as the new terminal barrier.
    const previousAttempt = client.getSnapshot().views[sessionId].snapshot?.attempt?.attempt_id;
    expect(typeof previousAttempt).toBe('string');
    if (typeof previousAttempt !== 'string') throw new Error('Settled Composer fixture lacks its native Attempt identity.');
    const secondDone = terminal(previousAttempt);
    const work = client.send(sessionId, ticket.text, [], 'send', ticket.accepted);
    const deliver = await received.promise;
    await secondDone;
    expect(client.getSnapshot().views[sessionId].inboundRequests).toBe(1);
    if (replacement === 'switch') {
      await expect(new CommandSession(client, sessionId, () => true).openNode(branch!.session.active_node, branch!.session.active_conversation_id)).rejects.toThrow('accepted inbound');
      expect(requests.filter(request => request.method === 'session/switchNode')).toHaveLength(0);
      // The lower lifecycle API retains explicit native Switch ownership; the
      // normal Web command above excludes this unresolved-inbound interleaving.
      await client.switchNode(sessionId, branch!.session.active_node); await client.attach(sessionId, branch!.session.active_node);
    } else { await client.release(sessionId); await client.attach(sessionId, created.session.active_node); }
    const next = client.target(sessionId), revision = owner.snapshot()!.revision;
    owner.activate('same-editor', [], 'new-proof', next); const before = requests.length;
    deliver(); await work;
    expect(ticket.outcome).toBe('accepted'); expect(ticket.acceptance?.message_id).toBeTruthy(); expect(ticket.conversation?.conversation_id).toBe(target.conversation_id);
    expect(next).not.toEqual(target); expect(next.conversation_id === target.conversation_id).toBe(replacement === 'reattach');
    expect(owner.snapshot()?.text).toBe(replacement === 'switch' ? 'Composer identity 1' : '');
    expect(owner.snapshot()?.incarnation).toBe(editor.incarnation);
    if (replacement === 'switch') expect(owner.snapshot()?.revision).toBe(revision);
    expect(requests.slice(before)).toEqual([]);
    expect(requests.filter(request => request.method === 'turn/start')).toHaveLength(2);
    expect(requests.filter(request => request.method === 'session/attach')).toHaveLength(2);
    expect(requests.filter(request => request.method === 'session/switchNode')).toHaveLength(replacement === 'switch' ? 1 : 0);
    passed = true;
  } finally { await client.disconnect(); await fixture.stop(passed); }
});
