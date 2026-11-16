// @vitest-environment node
import { afterEach, expect, it, vi } from 'vitest';
import { GEOMETRY_LIMITS } from '../src/app/components/documents/geometry/limits';

const module = new WebAssembly.Module(new Uint8Array([0,97,115,109,1,0,0,0]));
afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); vi.useRealTimers(); });
const load = async () => { vi.resetModules(); return (await import('../src/app/components/documents/geometry/cad-code')).cadCode; };

it('shares one in-flight and completed immutable code module across demands', async () => {
  const fetch = vi.fn().mockResolvedValue(new Response()); vi.stubGlobal('fetch', fetch);
  let complete!: (value: WebAssembly.Module) => void;
  const compile = vi.spyOn(WebAssembly, 'compileStreaming').mockReturnValue(new Promise(resolve => { complete = resolve; }));
  const code = await load(), first = code(), second = code();
  expect(first).toBe(second); expect(fetch).toHaveBeenCalledOnce(); expect(compile).toHaveBeenCalledOnce();
  expect(fetch.mock.calls[0][1]).toMatchObject({ credentials: 'same-origin', signal: expect.any(AbortSignal) });
  complete(module); await expect(first).resolves.toBe(module);
  expect(code()).toBe(first); expect(fetch).toHaveBeenCalledOnce();
});
it('retires a rejected compilation so explicit Retry can compile again', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response()));
  const compile = vi.spyOn(WebAssembly, 'compileStreaming').mockRejectedValueOnce(new WebAssembly.CompileError('Invalid asset')).mockResolvedValueOnce(module);
  const code = await load(); await expect(code()).rejects.toThrow('Invalid asset');
  await expect(code()).resolves.toBe(module); expect(compile).toHaveBeenCalledTimes(2);
});
it('aborts a stalled static asset fetch and permits the next explicit demand', async () => {
  vi.useFakeTimers();
  const fetch = vi.fn((_url: unknown, options: RequestInit) => new Promise<Response>((_resolve, reject) => options.signal!.addEventListener('abort', () => reject(options.signal!.reason), { once: true })));
  vi.stubGlobal('fetch', fetch);
  vi.spyOn(WebAssembly, 'compileStreaming').mockImplementation(async response => { await response; return module; });
  const code = await load(), pending = code(), rejected = expect(pending).rejects.toMatchObject({ name: 'AbortError' });
  await vi.advanceTimersByTimeAsync(GEOMETRY_LIMITS.timeoutMs); await rejected;
  expect(fetch.mock.calls[0][1].signal!.aborted).toBe(true);
  fetch.mockResolvedValueOnce(new Response()); await expect(code()).resolves.toBe(module);
  expect(fetch).toHaveBeenCalledTimes(2);
});
