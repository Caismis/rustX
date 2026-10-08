import {expect,it} from 'vitest';
import {ProtocolLog} from '../src/client/protocol-log';
it('ordinary protocol diagnostics omit even rejected literal authoring payloads',()=>{
 const log=new ProtocolLog();
 log.observe('out',1,JSON.stringify({jsonrpc:'2.0',id:1,method:'configuration/sourceWrite',params:{target:{kind:'user'},expected_revision:'revision',mutation:{kind:'mcp',id:'docs',authored:{definition:{headers:{Authorization:'private-sentinel'}}}}}}));
 const text=JSON.stringify(log.getSnapshot());
 expect(text).not.toContain('private-sentinel');expect(text).toContain('configuration write omitted');expect(text).toContain('revision');
});
