import open from 'open';
try {
  const child = await open(process.argv[2]);
  child.ref();
  const code = child.exitCode ?? await new Promise<number | null>((resolve, reject) => { child.once('error', reject); child.once('close', resolve); });
  if (code !== 0) process.exitCode = 1;
}
catch { process.exitCode = 1; }
