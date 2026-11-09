// @vitest-environment node
import { PassThrough } from 'node:stream';
import { existsSync, readdirSync } from 'node:fs';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
const boundary = vi.hoisted(() => ({ start: vi.fn(), failCleanup: false }));
vi.mock('../host/documents/office-sandbox.ts', () => ({ startOfficeSandbox: boundary.start }));
vi.mock('node:fs/promises', async original => {
  const actual = await original<typeof import('node:fs/promises')>();
  return { ...actual, rm: (...args: Parameters<typeof actual.rm>) => {
    if (boundary.failCleanup && String(args[0]).endsWith('.docx')) return Promise.reject(new Error('input cleanup denied'));
    return actual.rm(...args);
  } };
});
import { OfficeConverter } from '../host/documents/converter';
const owned: OfficeConverter[] = [];
const closing: (() => void)[] = [];
let reply: (input: PassThrough, output: PassThrough) => void;
function converter() { const value = new OfficeConverter(); owned.push(value); return value; }
function send(output: PassThrough, id: string, pdf = '%PDF-success', engine = 42) {
  output.write(JSON.stringify({ id, length: Buffer.byteLength(pdf) }) + '\n'); output.write(pdf);
  output.write(JSON.stringify({ id, settled: true, engine_pid: engine }) + '\n');
}
beforeEach(() => {
  boundary.start.mockReset(); boundary.failCleanup = false; reply = (input, output) => input.on('data', chunk => send(output, JSON.parse(String(chunk)).id));
  boundary.start.mockImplementation(async (_directory, _command, signal: AbortSignal) => {
    const input = new PassThrough(), output = new PassThrough();
    let settle!: () => void;
    const closed = new Promise<void>(resolve => { settle = resolve; });
    let stopped = false;
    const dispose = vi.fn(async () => { if (!stopped) { stopped = true; output.end(); settle(); } await closed; });
    signal.addEventListener('abort', () => void dispose(), { once: true });
    queueMicrotask(() => output.write('{"ready":true,"engine_pid":42}\n'));
    reply(input, output);
    return { input, output, pid: 123, closed, dispose, deadline: vi.fn(async () => {}) };
  });
});
afterEach(async () => { closing.splice(0).forEach(close => close()); await Promise.all(owned.splice(0).map(value => value.dispose())); vi.useRealTimers(); });
it('retains one converter process and private directory across different documents, then retires it on Host disposal', async () => {
  const engine = converter();
  expect(await engine.render(Buffer.from('first'), 'docx', new AbortController().signal)).toEqual(Buffer.from('%PDF-success'));
  const identity = await engine.identity, directory = boundary.start.mock.calls[0][0];
  expect(existsSync(directory)).toBe(true); expect(readdirSync(directory).some(name => name.endsWith('.docx'))).toBe(false);
  await engine.render(Buffer.from('different'), 'pptx', new AbortController().signal);
  expect(await engine.identity).toEqual(identity); expect(boundary.start).toHaveBeenCalledOnce();
  await engine.dispose(); expect(existsSync(directory)).toBe(false);
  await expect(engine.render(Buffer.from('late'), 'docx', new AbortController().signal)).rejects.toThrow('converter_unavailable');
});
it('a malformed result retires the old engine before a later document gets a fresh slot', async () => {
  reply = (input, output) => input.on('data', chunk => send(output, JSON.parse(String(chunk)).id, 'invalid PDF'));
  const engine = converter(); await expect(engine.render(Buffer.from('first'), 'docx', new AbortController().signal)).rejects.toThrow('converter_failure');
  const directory = boundary.start.mock.calls[0][0]; expect(existsSync(directory)).toBe(false);
  reply = (input, output) => input.on('data', chunk => send(output, JSON.parse(String(chunk)).id));
  await engine.render(Buffer.from('next'), 'pptx', new AbortController().signal); expect(boundary.start).toHaveBeenCalledTimes(2);
});
it('retires an idle engine crash before accepting a replacement document', async () => {
  const engine = converter(); await engine.warm();
  const old = await boundary.start.mock.results[0].value, directory = boundary.start.mock.calls[0][0];
  await old.dispose();
  await vi.waitFor(() => expect(existsSync(directory)).toBe(false));
  await engine.render(Buffer.from('new document'), 'pptx', new AbortController().signal);
  expect(boundary.start).toHaveBeenCalledTimes(2);
});
it('does not publish or reuse the slot when staged-input cleanup fails', async () => {
  const engine = converter(); boundary.failCleanup = true;
  await expect(engine.render(Buffer.from('input'), 'docx', new AbortController().signal)).rejects.toThrow('converter_unavailable');
  expect(existsSync(boundary.start.mock.calls[0][0])).toBe(false);
  await expect(engine.render(Buffer.from('next'), 'pptx', new AbortController().signal)).rejects.toThrow('converter_unavailable');
  expect(boundary.start).toHaveBeenCalledOnce();
});
it('publication waits for the settlement trailer, and cancellation cannot release the next seat early', async () => {
  let request!: () => void, finish!: () => void;
  const entered = new Promise<void>(resolve => { request = resolve; });
  const retired = new Promise<void>(resolve => { finish = resolve; }); closing.push(finish);
  reply = (input, output) => input.on('data', chunk => { const { id } = JSON.parse(String(chunk)); output.write(JSON.stringify({id,length:5})+'\n%PDF-'); request(); });
  const original = boundary.start.getMockImplementation()!;
  boundary.start.mockImplementation(async (...args) => {
    const sandbox = await original(...args);
    const immediate = sandbox.dispose;
    sandbox.dispose = async () => { await retired; await immediate(); };
    return sandbox;
  });
  const engine = converter(), abort = new AbortController(); let completed = false;
  const work = engine.render(Buffer.from('input'), 'docx', abort.signal).finally(() => { completed = true; });
  const rejected = expect(work).rejects.toThrow('obsolete');
  await entered; abort.abort(); await Promise.resolve(); expect(completed).toBe(false);
  await expect(engine.render(Buffer.from('other'), 'pptx', new AbortController().signal)).rejects.toThrow('capacity');
  finish(); await rejected; expect(existsSync(boundary.start.mock.calls[0][0])).toBe(false);
});
it('independently bounds each active document, while no document deadline runs on a healthy idle slot', async () => {
  vi.useFakeTimers();
  reply = () => {};
  const engine = converter(), work = engine.render(Buffer.from('input'), 'docx', new AbortController().signal);
  const rejected = expect(work).rejects.toThrow('converter_timeout');
  await vi.waitFor(() => expect(boundary.start).toHaveBeenCalledOnce());
  await vi.advanceTimersByTimeAsync(60000); await rejected; expect(vi.getTimerCount()).toBe(0);
});
