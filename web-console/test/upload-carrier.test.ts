import { expect, it } from 'vitest';
import { uploadUrl } from '../../protocol/app-server/upload';
const descriptor = { path: `/session-upload/${'a'.repeat(43)}`, loopback_port: null, expires_in_seconds: 60 };
it('resolves only a selected native origin or explicitly owned loopback descriptor', () => {
  expect(uploadUrl(descriptor, 'wss://native.example/')).toBe(`wss://native.example${descriptor.path}`);
  expect(uploadUrl({ ...descriptor, loopback_port: 2345 })).toBe(`ws://127.0.0.1:2345${descriptor.path}`);
  for (const path of ['https://foreign.example/upload', '//foreign.example/upload', '/other', `${descriptor.path}?key=secret`, `${descriptor.path}/../x`]) expect(() => uploadUrl({ ...descriptor, path }, 'wss://native.example/')).toThrow();
  for (const endpoint of ['https://native.example/', 'ws://user:secret@native.example/', 'ws://native.example/path', 'ws://native.example/?key=secret']) expect(() => uploadUrl(descriptor, endpoint)).toThrow();
  expect(() => uploadUrl({ ...descriptor, loopback_port: 2345 }, 'wss://native.example/')).toThrow();
  for (const port of [null, 0, -1, 65536, 1.5]) expect(() => uploadUrl({ ...descriptor, loopback_port: port })).toThrow();
});
