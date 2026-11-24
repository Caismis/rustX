import { fileURLToPath } from 'node:url';
import { parseArguments } from './arguments.ts';
import { Launcher } from './launcher.ts';

const root = fileURLToPath(new URL('../../', import.meta.url));
try {
  const input = process.argv.slice(2);
  const installed = input[0] === '--installed';
  if (installed) {
    input.shift();
    if (Number(process.versions.node.split('.')[0]) !== 24) throw new Error('The rustX bundle requires Node.js 24.');
  }
  const args = parseArguments(input, root);
  const launcher = new Launcher(root, undefined, undefined, installed);
  const interrupt = () => { void launcher.settle(130); };
  const hangup = () => { void launcher.settle(129); };
  const terminate = () => { void launcher.settle(143); };
  process.on('SIGINT', interrupt); process.on('SIGTERM', terminate); process.on('SIGHUP', hangup);
  try {
    const ready = await launcher.start(args);
    if (ready) {
      console.log(`[dev] Exact Workspace roots: ${ready.workspaces.join(', ')}`);
      launcher.handoff(ready, args.noOpen || !!process.env.SSH_CONNECTION || !!process.env.SSH_TTY);
    }
    process.exitCode = await launcher.done;
  } finally {
    process.off('SIGINT', interrupt); process.off('SIGTERM', terminate); process.off('SIGHUP', hangup);
  }
} catch (error) {
  console.error(`[dev] ${String(error)}`); process.exitCode = 2;
}
