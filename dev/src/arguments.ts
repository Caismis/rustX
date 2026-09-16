import { isAbsolute, resolve } from 'node:path';

export type Mode = 'app-server' | 'tui' | 'web';
export interface Arguments { mode: Mode; binary: string; forwarded: string[]; workspaces: string[] }

/** Only composition options are consumed. Native/TUI parsers validate their own grammar. */
export function parseArguments(argv: readonly string[], root: string): Arguments {
  const [mode, ...input] = argv;
  if (mode !== 'app-server' && mode !== 'tui' && mode !== 'web') throw new Error('Expected app-server, tui, or web');
  if (input[0] === '--') input.shift(); // pnpm run's separator
  let binary: string | undefined;
  const forwarded: string[] = [], workspaces: string[] = [];
  for (let i = 0; i < input.length; i++) {
    const flag = input[i];
    if (flag === '--binary' || (mode === 'web' && flag === '--workspace')) {
      const value = input[++i];
      if (!value || value.startsWith('--')) throw new Error(`${flag} requires a value`);
      if (flag === '--binary') {
        if (binary !== undefined) throw new Error('Duplicate --binary');
        binary = resolve(value);
      } else {
        if (!isAbsolute(value)) throw new Error('--workspace requires an absolute root');
        workspaces.push(value);
      }
    } else {
      if (mode === 'web' && (flag === '--listen' || flag === '--token-file')) throw new Error(`${flag} is owned by the Web composition`);
      forwarded.push(flag);
      // Native App Server options are pairs. Preserve opaque values even when
      // they happen to spell a launcher flag; do not reinterpret native syntax.
      if (mode !== 'tui' && i + 1 < input.length) forwarded.push(input[++i]);
    }
  }
  if (mode === 'web' && workspaces.length === 0) throw new Error('web requires at least one explicit --workspace');
  return { mode, binary: binary ?? resolve(root, 'target/debug/rustx'), forwarded, workspaces };
}
