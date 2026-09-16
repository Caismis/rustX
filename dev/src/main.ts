import { fileURLToPath } from 'node:url';
import { parseArguments } from './arguments.ts';
import { Launcher } from './launcher.ts';

const root = fileURLToPath(new URL('../../', import.meta.url));
try {
  const args = parseArguments(process.argv.slice(2), root);
  const launcher = new Launcher(root);
  const interrupt = () => { void launcher.settle(130); };
  const terminate = () => { void launcher.settle(143); };
  process.on('SIGINT', interrupt); process.on('SIGTERM', terminate);
  try {
    const ready = await launcher.start(args);
    if (ready) console.log(`[dev] Browser: ${ready.url}\n[dev] App Server: ${ready.endpoint}\n[dev] Transport token file (enter its contents in Connect): ${ready.tokenFile}\n[dev] Exact Workspace roots: ${ready.workspaces.join(', ')}`);
    process.exitCode = await launcher.done;
  } finally {
    process.off('SIGINT', interrupt); process.off('SIGTERM', terminate);
  }
} catch (error) {
  console.error(`[dev] ${String(error)}`); process.exitCode = 2;
}
