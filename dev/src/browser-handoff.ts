import open from 'open';
import type { ChildProcess } from 'node:child_process';

/** OS acceptance, never ownership of the user's browser lifetime. */
export async function browserHandoff(url: string, platform = process.platform, launch: (url: string) => Promise<ChildProcess> = open) {
  const launcher = await launch(url);
  if (platform !== 'win32') return;
  // Windows open() resolves at PowerShell spawn, before URL dispatch completes.
  const code = launcher.exitCode ?? await new Promise<number | null>((resolve, reject) => {
    const closed = (code: number | null) => { launcher.off('error', failed); resolve(code); };
    const failed = (error: Error) => { launcher.off('close', closed); reject(error); };
    launcher.ref();
    launcher.once('error', failed);
    launcher.once('close', closed);
  });
  if (code !== 0) throw new Error('Operating-system browser launcher failed');
}
