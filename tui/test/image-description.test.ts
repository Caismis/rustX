import assert from 'node:assert/strict';
import { test } from 'node:test';
import { rendererFor } from '../src/ui/components/tool-renderers.ts';

test('Bash safely renders intent and retains the actual command in detail', () => {
 const args = { command: 'printf authoritative', description: '\u001b[2J<script>intent</script>\u0007' };
 const rendered = rendererFor('tool-bash').renderCall?.(args);
 assert.ok(rendered);
 assert.ok(!rendered.subject?.includes('\u001b'));
 assert.ok(!rendered.subject?.includes('\u0007'));
 assert.match(rendered.detail?.join('\n') ?? '', /printf authoritative/);
 assert.deepEqual(rendererFor('tool-bash').renderCall?.(JSON.parse(JSON.stringify(args))), rendered);
});
test('image reads use dedicated presentation and canonical artifact identity', () => {
 const renderer = rendererFor('tool-read-image');
 assert.equal(renderer.renderCall?.({ path: 'sample.png' })?.title, 'Read image');
 const result = { content: [{ type: 'image' as const, artifact_id: 'artifact_1' }], deliveries: [] };
 assert.match(renderer.renderResult?.(result, {})?.detail?.join('\n') ?? '', /artifact_1/);
});
