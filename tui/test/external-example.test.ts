/** The standalone example runs against the actual process in the normal TUI CI lane. */
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {existsSync,mkdtempSync,mkdirSync,writeFileSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {createInterface} from 'node:readline';
import {once} from 'node:events';
import {test} from 'node:test';
const binary=process.env.RUSTX_BINARY??fileURLToPath(new URL('../../target/debug/rustx',import.meta.url));
if(!existsSync(binary)&&process.env.RUSTX_REQUIRE_PROVIDER_EMULATOR)throw Error('Build the actual rustx binary before external example validation.');
test('standalone connect/initialize/read example uses the public authenticated WebSocket', {skip:!existsSync(binary),timeout:120_000},async()=>{
  const root=mkdtempSync(join(tmpdir(),'rustx-external-example-'));
  mkdirSync(join(root,'home'));writeFileSync(join(root,'rustx.toml'),'');
  const token='standalone/+example==';writeFileSync(join(root,'token'),token,{mode:0o600});
  const server=spawn(binary,['app-server','--config',join(root,'rustx.toml'),'--runtime-root',join(root,'runtime'),'--listen','ws://127.0.0.1:0','--token-file',join(root,'token')],{env:{...process.env,HOME:join(root,'home'),XDG_CONFIG_HOME:undefined,XDG_STATE_HOME:undefined},stdio:['ignore','ignore','pipe']});
  const ended=once(server,'exit');
  try {
    const endpoint=await new Promise<string>((resolve,reject)=>{
      const lines=createInterface({input:server.stderr});
      lines.on('line',line=>{if(line.startsWith('rustx app-server listening '))resolve(line.slice('rustx app-server listening '.length));});
      server.once('exit',code=>reject(Error(`App Server exited before readiness: ${code}`)));
    });
    const example=spawn(process.execPath,[fileURLToPath(new URL('../../examples/app-server/connect.mjs',import.meta.url)),endpoint,join(root,'token')],{stdio:['ignore','pipe','pipe']});
    let output='',error='';example.stdout.on('data',chunk=>output+=chunk);example.stderr.on('data',chunk=>error+=chunk);
    const [code]=await once(example,'exit');assert.equal(code,0,error);
    assert.equal(JSON.parse(output).type,'server_info');assert.ok(!output.includes(token));
  } finally {
    server.kill('SIGTERM');await ended;rmSync(root,{recursive:true,force:true});
  }
});
