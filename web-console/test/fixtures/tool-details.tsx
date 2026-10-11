import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Tool } from '../../src/app/agent/Tool';
import type { ForegroundToolExecution } from '../../../protocol/app-server/v44';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/gradient-shadow-text.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/reset.css';

const command = Array.from({ length: 36 }, (_, index) => `    printf 'command ${index + 1}\\n'`).join('\n');
const output = Array.from({ length: 48 }, (_, index) => `result ${index + 1}    preserved alignment`).join('\n') + '\n';
const tool = (id: string, name: string, args: object, text: string, toolId = 'tool-bash'): ForegroundToolExecution => ({
  message_id: `message-${id}`, call_id: id, block_index: 0, tool_id: toolId, name,
  state: { type: 'settled', arguments: JSON.stringify(args), result: { status: { type: 'success' }, exit_code: 0, duration_ms: 4, content: [{ type: 'text', text }] } },
});
const bash = tool('long-bash', 'bash', { command, description: 'Apply corrections and back up the document' }, output);
const generic = tool('generic', 'exa_search', { query: Array.from({ length: 40 }, (_, index) => `query ${index + 1}`) }, output, 'mcp.exa.search');
const read = tool('read', 'read', { path: '/workspace/report.md' }, 'one\ntwo\nthree', 'tool-read');
const edit = tool('edit', 'edit', { path: '/workspace/report.md', edits: [{ oldText: 'one', newText: 'two' }] }, 'Updated report.md', 'tool-edit');
const search = tool('search', 'grep', { pattern: 'report' }, 'report.md:1:one\nreport.md:2:two', 'tool-grep');
function Fixture() {
  const [running, setRunning] = useState(false);
  return <main style={{ maxWidth: 760, margin: '24px auto', padding: 16 }}>
    <h1>Tool details</h1>
    <button type="button" onClick={() => setRunning(value => !value)}>Toggle running</button>
    <Tool tool={running ? { ...bash, state: { type: 'running', arguments: bash.state.arguments } } : bash}/>
    <Tool tool={generic}/><Tool tool={read}/><Tool tool={edit}/><Tool tool={search}/>
    <Tool tool={{ ...tool('failure', 'bash', { command: 'false', description: 'Original description' }, 'Native failure'), state: { type: 'settled', arguments: '{"command":"false"}', result: { status: { type: 'failed', error: 'Native failure' }, exit_code: 1, duration_ms: 4 } } }}/>
  </main>;
}
createRoot(document.getElementById('root')!).render(<Fixture/>);
