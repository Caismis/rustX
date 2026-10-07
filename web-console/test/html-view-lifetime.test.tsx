import { act, cleanup, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { HtmlBody } from '../src/app/components/workbench-documents/html/HtmlBody';
import type { WorkbenchCall } from '../src/app/components/WorkbenchFiles';
vi.mock('../src/app/components/workbench-documents/html/preferences', () => ({ useInteractiveHtml: () => [true] }));
afterEach(() => { cleanup(); vi.restoreAllMocks(); });
it('closing during dependency reads cancels authority and discards the late frame', async () => {
  let release!: (value: {base64:string}) => void;
  const promise = new Promise<{base64:string}>(resolve => { release = resolve; });
  let reading: AbortSignal | undefined;
  const call: WorkbenchCall = (_request, signal) => { reading = signal; return promise; };
  const create = vi.spyOn(URL, 'createObjectURL');
  const body=render(<HtmlBody data={new TextEncoder().encode('<script src="app.js"></script>')} path="docs/index.html" call={call} signal={new AbortController().signal}/>);
  expect(reading?.aborted).toBe(false);
  body.unmount();
  expect(reading?.aborted).toBe(true);
  await act(async()=>release({base64:btoa('window.ready=true')}));
  expect(create).not.toHaveBeenCalled();
});
it('retires a published iframe URL on unmount', async () => {
  const create=vi.spyOn(URL,'createObjectURL').mockReturnValue('blob:test');
  const revoke=vi.spyOn(URL,'revokeObjectURL').mockImplementation(()=>{});
  const body=render(<HtmlBody data={new TextEncoder().encode('<h1>Preview</h1>')} path="index.html" call={vi.fn()} signal={new AbortController().signal}/>);
  await act(async()=>{});
  expect(create).toHaveBeenCalledOnce();
  expect(body.container.querySelector('iframe')?.getAttribute('sandbox')).toBe('allow-scripts');
  body.unmount();expect(revoke).toHaveBeenCalledWith('blob:test');
});
