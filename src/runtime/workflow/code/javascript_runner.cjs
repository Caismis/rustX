// Invoke a Workflow main with typed inputs using workspace module resolution.
const fs = require('node:fs');
const payload = JSON.parse(fs.readFileSync(0, 'utf8'));
const writeResult = process.stdout.write.bind(process.stdout);
process.stdout.write = process.stderr.write.bind(process.stderr);
(async () => {
  const main = new Function('require', `${payload.code}\nreturn main;`)(require);
  if (typeof main !== 'function') throw new TypeError('Code must define main');
  const result = await main(payload.inputs);
  if (!result || typeof result !== 'object' || Array.isArray(result))
    throw new TypeError('main must return an object');
  const json = JSON.stringify(result, (_key, value) => {
    if (typeof value === 'number' && !Number.isFinite(value))
      throw new TypeError('main must return finite JSON numbers');
    return value;
  });
  writeResult(json);
})().catch(error => { console.error(error); process.exitCode = 1; });
