import { createHash } from 'node:crypto';
import { mkdirSync, realpathSync } from 'node:fs';
import { homedir } from 'node:os';
import { basename, isAbsolute, join } from 'node:path';

/** Durable product metadata, separate from native runtime and secret scratch.
 * Opaque launch arguments distinguish profiles without resolving Native config. */
export function workspaceRegistry(workspaces: readonly string[], forwarded: readonly string[], stateDirectory?: string) {
  const paths = workspaces.map(path => realpathSync(path));
  const hash = (value: unknown) => createHash('sha256').update(JSON.stringify(value)).digest('hex');
  const stateHome = process.env.XDG_STATE_HOME;
  const base = stateDirectory ?? join(stateHome && isAbsolute(stateHome) ? stateHome : join(homedir(), '.local', 'state'), 'rustx', 'web');
  const directory = join(base, hash({ workspaces: [...paths].sort(), forwarded }));
  mkdirSync(directory, { recursive: true, mode: 0o700 });
  return { metadataFile: join(directory, 'workspaces.json'), roots: paths.map(cwd => ({ id: hash(cwd), cwd, displayName: basename(cwd) || cwd })) };
}
