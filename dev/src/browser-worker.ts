import { browserHandoff } from './browser-handoff.ts';
try {
  await browserHandoff(process.argv[2]);
}
catch { process.exitCode = 1; }
