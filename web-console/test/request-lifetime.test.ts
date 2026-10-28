import { jobReceipt, compactReceipt } from './fixture';
import { afterEach, expect, it, vi } from 'vitest';
import type { MethodResult, Request1 } from '../../protocol/app-server/v62';
import { OutcomeUncertain, RequestNotDispatched } from '../src/client/errors';
import { Server, snapshot } from './fixture';
import { firstSubmitPort } from '../src/app/new-conversation/port';
const servers: Server[] = [];
afterEach(() => { for (const s of servers.splice(0)) s.client.disconnect(); vi.useRealTimers(); });
async function connected() { const s = new Server(); servers.push(s); await s.attached('A'); return s; }
type DomainMethod = 'turn/cancel' | 'agent/wait' | 'job/wait' | 'agent/interrupt' | 'job/cancel' | 'agent/sendMessage' | 'context/compact';
function operation(s: Server, method: DomainMethod): Request1 {
  const target = s.client.target('A');
  if (method === 'context/compact') return { method, params: { target, request_id: 'compact-a' } };
  if (method === 'turn/cancel') return { method, params: { target } };
  return method === 'job/wait' || method === 'job/cancel' ? { method, params: { target, job_id: 'job-a' } }
    : method === 'agent/sendMessage' ? { method, params: { target, agent_id: 'agent-a', attachments: [], message: 'input' } }
    : { method, params: { target, agent_id: 'agent-a' } };
}
function result(method: DomainMethod): MethodResult {
  if (method === 'context/compact') return compactReceipt({ compaction_count: 1, compaction_in_progress: false });
  if (method === 'turn/cancel') return { type: 'cancellation_accepted', attempt_id: 'attempt-A' };
  if (method === 'job/cancel') return { type: 'job_cancellation_accepted', job_id: 'job-a' };
  if (method === 'agent/interrupt') return { type: 'agent_interruption_accepted', agent_id: 'agent-a', activation_id: 'activation-b' };
  if (method === 'job/wait') return jobReceipt({ job_id: 'job-a', tool_id: 'bash', tool_name: 'bash', state: 'cancelled' });
  if (method === 'agent/sendMessage') return { type: 'agent_message', agent_id: 'agent-a', activation_id: 'activation-b', resumed: true };
  return { type: 'agent_wait', agent_id: 'agent-a', activation_id: 'activation-b', outcome: null };
}
const methods: DomainMethod[] = ['agent/wait', 'job/wait', 'agent/sendMessage', 'agent/interrupt', 'job/cancel'];
it('a physically closing socket refuses an unsent mutation before its close event arrives', async () => {
  const s = await connected();
  const before = s.client.getSnapshot().views.A.snapshot;
  s.socket.readyState = 2;
  await expect(s.client.request(operation(s, 'agent/sendMessage'), 'agent_message')).rejects.toBeInstanceOf(RequestNotDispatched);
  expect(s.requests.filter(row => row.request.method === 'agent/sendMessage')).toHaveLength(0);
  expect(s.client.getSnapshot().uncertain).toEqual([]);
  expect(s.client.getSnapshot().views.A.snapshot).toBe(before);
});
it('retiring a child observation cancels its exact native read, while leaving Agent execution untouched', async () => {
  const s = await connected(); s.held.add('agent/observe');
  const controller = new AbortController();
  const work = s.client.request({method:'agent/observe',params:{target:s.client.target('A'),agent_id:'agent-a',after:null}},
    'agent_observe',undefined,undefined,controller.signal);
  const rejected = expect(work).rejects.toThrow('retired child observation');
  const read = await s.waitFor('agent/observe',1);
  controller.abort();
  const cancel = await s.waitFor('agent/observeCancel',1);
  expect(cancel.params).toEqual({request_id:read.id});
  s.socket.deliver({jsonrpc:'2.0',id:read.id,error:{code:-32000,message:'retired child observation',data:{kind:'observation_cancelled'}}});
  await rejected;
  expect(s.requests.some(row => row.request.method === 'agent/interrupt')).toBe(false);
  const queuedController = new AbortController();
  const reads=Array.from({length:8},(_,index)=>s.client.request({method:'agent/observe',params:{target:s.client.target('A'),agent_id:`agent-${index}`,after:null}},'agent_observe'));
  for(const read of reads)void read.catch(()=>{});
  await s.waitFor('agent/observe',9);
  const queued = s.client.request({method:'agent/observe',params:{target:s.client.target('A'),agent_id:'queued',after:null}},'agent_observe',undefined,undefined,queuedController.signal);
  const queuedRejected=expect(queued).rejects.toThrow('retired before dispatch');queuedController.abort();await queuedRejected;
  expect(s.requests.filter(row=>row.request.method==='agent/observe')).toHaveLength(9);
  expect(s.requests.filter(row=>row.request.method==='agent/observeCancel')).toHaveLength(1);
  s.client.disconnect();
});
it.each([25_000, 61_000])('attachment remains pending for %i ms before first Send and ten seconds of native readiness wait', async beforeSubmission => {
  const s = new Server(); servers.push(s); await s.connect();
  s.held.add('session/attach'); vi.useFakeTimers();
  const opening = s.client.attach('A'); void opening.catch(() => {});
  const attachment = await s.waitFor('session/attach', 1);
  await vi.advanceTimersByTimeAsync(beforeSubmission);
  expect(s.client.getSnapshot().connection).toBe('connected');
  expect(s.requests.filter(row => ['session/submissionIntent', 'turn/start'].includes(row.request.method))).toEqual([]);
  expect(s.client.getSnapshot().uncertain).toEqual([]);
  const port = firstSubmitPort(s.client, s.workspaceHost, () => true, () => () => {}, true);
  const submitted = port.readExisting!('A'); void submitted.catch(() => {});
  const intent = await s.waitFor('session/submissionIntent', 1);
  expect(intent.params).toEqual({ session_id: 'A', node_id: 'node-A' });
  await vi.advanceTimersByTimeAsync(10_000);
  expect(s.client.getSnapshot().connection).toBe('connected');
  expect(s.client.getSnapshot().views.A.attachment).toBe('attaching');
  expect(s.client.getSnapshot().uncertain).toEqual([]);
  expect(s.requests.filter(row => row.request.method === 'turn/start')).toEqual([]);
  s.reply(attachment); await opening;
  await expect(submitted).resolves.toEqual({ id: 'A', node: 'node-A', conversation: 'conversation-A' });
  await s.client.send('A', 'one authorized first input');
  expect(s.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(1);
  expect(s.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(1);
});

it('the attachment lifecycle lane rejects its thirty-third waiter without starving ordinary or metadata requests', async () => {
  const s = new Server(); servers.push(s); await s.connect();
  s.held.add('session/attach');
  const openings: Promise<void>[] = [];
  for (let index = 0; index < 32; index++) {
    const id = `cold-${index}`; s.snapshots.set(id, snapshot(id));
    const opening = s.client.attach(id); void opening.catch(() => {});
    openings.push(opening);
    await s.waitFor('session/attach', index + 1);
  }
  s.snapshots.set('overflow', snapshot('overflow'));
  await expect(s.client.attach('overflow')).rejects.toThrow('attachment capacity');
  expect(s.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(32);
  await expect(s.client.request({ method: 'server/info', params: {} }, 'server_info')).resolves.toMatchObject({ type: 'server_info' });
  await expect(s.client.request({ method: 'session/settings', params: { session_id: 'cold-0' } }, 'settings')).resolves.toMatchObject({ type: 'settings' });
  expect(s.loaded.size).toBe(0);
  expect(s.client.getSnapshot().uncertain).toEqual([]);
  s.client.disconnect(); await Promise.allSettled(openings);
});

it('real attachment response loss remains one uncertain mutation after an unbounded initialization wait', async () => {
  const s = new Server(); servers.push(s); await s.connect();
  s.held.add('session/attach'); vi.useFakeTimers();
  const opening = s.client.attach('A');
  const rejected = expect(opening).rejects.toBeInstanceOf(OutcomeUncertain);
  const request = await s.waitFor('session/attach', 1);
  await vi.advanceTimersByTimeAsync(120_000);
  expect(s.client.getSnapshot().connection).toBe('connected');
  s.commit(request); // Native claim committed; acknowledgement is deliberately lost.
  s.socket.close(); await rejected;
  expect(s.client.getSnapshot().uncertain).toEqual([expect.objectContaining({ method: 'session/attach', sessionId: 'A' })]);
  expect(s.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(1);
  expect(s.requests.filter(row => row.request.method === 'turn/start')).toEqual([]);
});

it.each([...methods, 'turn/cancel', 'context/compact'] as const)('%s owns its lifetime without incidental traffic', async method => {
  const s = await connected(); s.held.add(method); vi.useFakeTimers();
  const settled = vi.fn(); const response = result(method);
  const work = s.client.request(operation(s, method), response.type).then(settled);
  void work.catch(() => {}); // Baseline failure remains asserted without an unhandled rejection.
  const request = await s.waitFor(method, 1);
  await vi.advanceTimersByTimeAsync(method==='agent/sendMessage'?1_000:120_000);
  expect(settled).not.toHaveBeenCalled(); expect(s.client.getSnapshot().connection).toBe('connected');
  await s.client.request({ method: 'session/settings', params: { session_id: 'A' } }, 'settings');
  s.socket.success(request, response); await work;
  s.socket.success(request, response); await Promise.resolve();
  expect(settled).toHaveBeenCalledExactlyOnceWith(response);
});
it.each(['agent/wait', 'job/wait', 'context/compact'] as const)('%s at full capacity stays healthy with notifications and permits controls', async method => {
  const s = await connected(); s.held.add(method); vi.useFakeTimers();
  const response = result(method); const settled = vi.fn();
  const waits = Array.from({ length: 6 }, () => s.client.request(operation(s, method), response.type).then(settled));
  expect(s.requests.filter(row => row.request.method === method)).toHaveLength(6);
  const queued=s.client.request(operation(s,method),response.type);void queued.catch(()=>{});
  expect(s.requests.filter(row=>row.request.method===method)).toHaveLength(6);
  for(let i=0;i<4;i++){s.cursor++;s.socket.deliver({jsonrpc:'2.0',method:'session/event',params:{target:s.client.target('A'),cursor:String(s.cursor),event:{type:'goal_changed',view:{current:null}}}});await vi.advanceTimersByTimeAsync(30_001);}
  expect(settled).not.toHaveBeenCalled(); expect(s.client.getSnapshot().connection).toBe('connected');
  for (const control of ['turn/cancel'] as const) {
    s.handlers.set(control, () => result(control));
    await expect(s.client.request(operation(s, control), result(control).type)).resolves.toEqual(result(control));
  }
  const inspect=s.client.request({method:'session/settings',params:{session_id:'A'}},'settings');void inspect.catch(()=>{});
  for (const { request } of s.requests.filter(row => row.request.method === method)) s.socket.success(request, response);
  const queuedRequest=await s.waitFor(method,7);s.socket.success(queuedRequest,response);await Promise.all([...waits,queued,inspect]);expect(settled).toHaveBeenCalledTimes(6);
});
it('T20 six ordinary waits leave both finite control slots independent',async()=>{
 const s=await connected();s.held.add('agent/wait');s.held.add('turn/cancel');
 const work=Array.from({length:6},()=>s.client.request(operation(s,'agent/wait'),'agent_wait'));
 const controls=Array.from({length:2},()=>s.client.request(operation(s,'turn/cancel'),'cancellation_accepted'));
 const queued=s.client.request(operation(s,'agent/wait'),'agent_wait');void queued.catch(()=>{});expect(s.requests.filter(row=>row.request.method==='agent/wait')).toHaveLength(6);
 await expect(s.client.request(operation(s,'turn/cancel'),'cancellation_accepted')).rejects.toThrow('control capacity');
 for(const {request}of s.requests.filter(row=>s.held.has(row.request.method)))s.socket.success(request,result(request.method as DomainMethod));
 const extra=await s.waitFor('agent/wait',7);s.socket.success(extra,result('agent/wait'));await Promise.all([...work,...controls,queued]);expect(s.client.getSnapshot().connection).toBe('connected');
});
it('two metadata requests bypass full ordinary capacity and release only their own response slots', async () => {
  const s = await connected();
  for (const method of ['agent/wait', 'session/read', 'session/settings'] as const) s.held.add(method);
  const waits = Array.from({ length: 6 }, () => s.client.request(operation(s, 'agent/wait'), 'agent_wait'));
  const readCount = s.requests.filter(row => row.request.method === 'session/read').length;
  const settingsCount = s.requests.filter(row => row.request.method === 'session/settings').length;
  const first = s.client.request({ method: 'session/read', params: { session_id: 'A' } }, 'session');
  const second = s.client.request({ method: 'session/settings', params: { session_id: 'A' } }, 'settings');
  const third = s.client.request({ method: 'session/read', params: { session_id: 'B' } }, 'session');
  expect(s.requests.filter(row => row.request.method === 'session/read')).toHaveLength(readCount + 1);
  expect(s.requests.filter(row => row.request.method === 'session/settings')).toHaveLength(settingsCount + 1);
  s.handlers.set('turn/cancel', () => result('turn/cancel'));
  await s.client.request(operation(s, 'turn/cancel'), 'cancellation_accepted');
  const firstRead = s.requests.filter(row => row.request.method === 'session/read').at(-1)!.request;
  s.reply(firstRead);
  const queuedRead = await s.waitFor('session/read', readCount + 2);
  expect(queuedRead.params).toEqual({ session_id: 'B' });
  expect(s.requests.filter(row => row.request.method === 'agent/wait')).toHaveLength(6);
  s.reply(queuedRead);
  s.reply(s.requests.filter(row => row.request.method === 'session/settings').at(-1)!.request);
  for (const { request } of s.requests.filter(row => row.request.method === 'agent/wait')) s.socket.success(request, result('agent/wait'));
  await Promise.all([first, second, third, ...waits]);
  expect(s.client.getSnapshot().connection).toBe('connected');
});
it.each(['session/read', 'session/settings', 'session/create'] as const)('%s retains its finite response deadline in the metadata lane', async method => {
  const s = await connected(); s.held.add(method); vi.useFakeTimers();
  const work = method === 'session/create'
    ? s.client.request({ method, params: { settings: { cwd: '/workspace/A' } } }, 'session_transition')
    : method === 'session/read'
      ? s.client.request({ method, params: { session_id: 'A' } }, 'session')
      : s.client.request({ method, params: { session_id: 'A' } }, 'settings');
  const rejected = method === 'session/create'
    ? expect(work).rejects.toBeInstanceOf(OutcomeUncertain)
    : expect(work).rejects.toThrow('Disconnected before a response');
  await vi.advanceTimersByTimeAsync(29_999);
  expect(s.client.getSnapshot().connection).toBe('connected');
  await vi.advanceTimersByTimeAsync(1);
  await rejected;
  expect(s.client.getSnapshot().connection).not.toBe('connected');
  expect(s.client.getSnapshot().uncertain).toEqual(method === 'session/create' ? [expect.objectContaining({ method })] : []);
});
it.each(['close', 'error'] as const)('socket %s settles operations once without replay or stale adoption', async failure => {
  const s = await connected(); for (const method of methods) s.held.add(method);
  const rejected = vi.fn();
  const work = methods.map(method => s.client.request(operation(s, method), result(method).type).catch(error => { rejected(error); return error; }));
  const old = s.socket; const requests = s.requests.filter(row => s.held.has(row.request.method));
  if (failure === 'close') old.close(); else old.onerror?.(new Event('error'));
  const errors = await Promise.all(work);
  expect(errors.slice(0, 2).every(error => !(error instanceof OutcomeUncertain))).toBe(true);
  expect(errors.slice(2).every(error => error instanceof OutcomeUncertain)).toBe(true);
  expect(rejected).toHaveBeenCalledTimes(5);
  await s.connect(); const current = s.client.getSnapshot();
  for (const { request } of requests) old.success(request, result(request.method as DomainMethod));
  expect(s.client.getSnapshot()).toBe(current);
  for (const method of methods) expect(s.requests.filter(row => row.request.method === method)).toHaveLength(1);
  expect(s.client.getSnapshot().uncertain.map(row => row.method)).toEqual(methods.slice(2));
});

it('full wait and RPC lanes preserve controls and retired startup dispatch ownership', async () => {
  const s = await connected(); s.held.add('agent/wait'); s.held.add('server/info');
  const waits = Array.from({ length: 3 }, () => s.client.request(operation(s, 'agent/wait'), 'agent_wait'));
  const reads = Array.from({ length: 3 }, () => s.client.request({ method: 'server/info', params: {} }, 'server_info'));
  let current = true;
  const admission = s.client.send('A', 'retained', [], 'send', undefined, () => current);
  const rejected = expect(admission).rejects.toThrow('before dispatch');
  expect(s.requests.filter(row=>row.request.method==='agent/wait')).toHaveLength(3);
  for (const control of ['turn/cancel'] as const) {
    s.handlers.set(control, () => result(control));
    await expect(s.client.request(operation(s, control), result(control).type)).resolves.toEqual(result(control));
  }
  current = false;
  for (const item of s.requests.filter(item => item.request.method === 'server/info').slice(-3)) s.reply(item.request);
  await Promise.all([...reads, rejected]);
  expect(s.requests.filter(item => item.request.method === 'turn/start')).toEqual([]);
  expect(s.client.getSnapshot().uncertain).toEqual([]);
  expect(s.client.getSnapshot().connection).toBe('connected');
  for (const item of s.requests.filter(item => item.request.method === 'agent/wait')) s.socket.success(item.request, result('agent/wait'));
  await Promise.all(waits);
});

it('a missing finite Agent input receipt expires as unknown delivery without replay or fabricated execution settlement',async()=>{
 const s=await connected();s.held.add('agent/sendMessage');vi.useFakeTimers();
 const before=s.client.getSnapshot().views.A.snapshot;
 const result=s.client.request(operation(s,'agent/sendMessage'),'agent_message').catch(error=>error);
 await s.waitFor('agent/sendMessage',1);await vi.advanceTimersByTimeAsync(120_000);
 expect(await result).toBeInstanceOf(OutcomeUncertain);
 expect(s.client.getSnapshot().uncertain.map(row=>row.method)).toEqual(['agent/sendMessage']);
 expect(s.client.getSnapshot().views.A.snapshot).toBe(before);
 expect(s.requests.filter(row=>row.request.method==='agent/sendMessage')).toHaveLength(1);
 expect(s.requests.some(row=>row.request.method==='agent/interrupt')).toBe(false);
});
