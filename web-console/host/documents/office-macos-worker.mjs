/** Runs only inside office-macos.ts's clean-environment Seatbelt process group. */
import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
const [entry, directory, extension] = process.argv.slice(2);
const { createConverter } = await import(pathToFileURL(entry).href);
const converter = await createConverter({
  fontMetadataCacheDirectory: false,
  fontDirectories: ['/System/Library/Fonts', '/Library/Fonts'],
  timeoutMs: 15000,
  maxInputBytes: 512 * 1024,
  maxOutputBytes: 4 * 1024 * 1024,
  maxArchiveEntries: 256,
  maxUncompressedBytes: 16 * 1024 * 1024,
  maxImageResolution: 144,
  // Keep the engine's bounded font defaults, including large CJK system fonts.
});
try {
  await converter.render({ inputPath: `${directory}/source.${extension}`, outputPath: `${directory}/result.pdf` });
  const result = await readFile(`${directory}/result.pdf`);
  if (result.length > 4 * 1024 * 1024) throw new Error('too_large');
  process.stdout.write(result);
} finally {
  await converter.dispose();
}
