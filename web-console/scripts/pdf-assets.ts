/** Version-matched PDF.js resources, bundled with the lazy reader instead of fetched by a document. */
import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
export function pdfAssets() {
  const root = new URL('../', import.meta.resolve('pdfjs-dist'));
  const files = (directory: string, extension: RegExp) => Object.fromEntries(readdirSync(new URL(directory, root))
    .filter(name => extension.test(name)).map(name => [name, readFileSync(fileURLToPath(new URL(`${directory}/${name}`, root))).toString('base64')]));
  return { cMapUrl: files('cmaps', /\.bcmap$/), standardFontDataUrl: files('standard_fonts', /\.(?:pfb|ttf)$/), wasmUrl: {} };
}
