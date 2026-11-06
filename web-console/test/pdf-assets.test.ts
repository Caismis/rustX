// @vitest-environment node
import { expect, it, vi } from 'vitest';
import { pdfAssets } from '../scripts/pdf-assets';
import { createPdfBinaryDataFactory } from '../src/client/pdf-assets';
it('bundles the installed PDF.js standard fonts and character maps and decodes independent bytes', async () => {
  const assets = pdfAssets(), Factory = createPdfBinaryDataFactory(assets), factory = new Factory();
  expect(Object.keys(assets.cMapUrl).length).toBeGreaterThan(100);
  expect(Object.keys(assets.standardFontDataUrl).length).toBeGreaterThan(10);
  const filename = Object.keys(assets.standardFontDataUrl)[0];
  const first = await factory.fetch({ kind: 'standardFontDataUrl', filename });
  expect(Buffer.from(first)).toEqual(Buffer.from(assets.standardFontDataUrl[filename], 'base64'));
  first[0] ^= 255;
  expect(await factory.fetch({ kind: 'standardFontDataUrl', filename })).not.toEqual(first);
});
it.each(['https://example.invalid/font.ttf', '../font.ttf', 'toString', 'missing.ttf'])('missing PDF asset %s never falls back to the network', async filename => {
  const fetch = vi.spyOn(globalThis, 'fetch');
  const Factory = createPdfBinaryDataFactory({ standardFontDataUrl: {}, cMapUrl: {}, wasmUrl: {} });
  await expect(new Factory().fetch({ kind: 'standardFontDataUrl', filename })).rejects.toThrow('not bundled');
  expect(fetch).not.toHaveBeenCalled();
});
