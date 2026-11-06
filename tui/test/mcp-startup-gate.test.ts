import assert from 'node:assert/strict';
import { it } from 'node:test';
import { mcpStartupGate } from './support/mcp-startup-gate.ts';
import { TempFixture } from './support/temp-fixture.ts';

it('MCP discovery wait reports early attachment failure or completion', async () => {
  const fixture = TempFixture.create('rustx-tui-gate-outcome-');
  const controller = new AbortController();
  const gate = await mcpStartupGate(fixture.root, controller.signal);
  try {
    const failed = new Error('native attachment failed before tools/list');
    await assert.rejects(gate.waitForDiscovery(Promise.reject(failed)), error => error === failed);
    await assert.rejects(gate.waitForDiscovery(Promise.resolve()), /Attachment completed before the held MCP discovery/);
    assert.equal(gate.calls, 0);
  } finally {
    await gate.close();
    fixture.cleanup();
  }
});

it('test cancellation rejects pending discovery and closes its owned gate', async () => {
  const fixture = TempFixture.create('rustx-tui-gate-abort-');
  const controller = new AbortController();
  const gate = await mcpStartupGate(fixture.root, controller.signal);
  try {
    const cancelled = new Error('test watchdog cancelled');
    const waiting = gate.waitForDiscovery(new Promise(() => {}));
    const rejected = assert.rejects(waiting, error => error === cancelled);
    controller.abort(cancelled);
    await rejected;
    await gate.close();
    assert.equal(gate.calls, 0);
  } finally {
    await gate.close();
    fixture.cleanup();
  }
});

it('closing an unused MCP gate rejects every pending discovery watcher', async () => {
  const fixture = TempFixture.create('rustx-tui-gate-close-');
  const gate = await mcpStartupGate(fixture.root, new AbortController().signal);
  try {
    const first = assert.rejects(gate.waitForDiscovery(new Promise(() => {})), /gate closed before discovery/);
    const second = assert.rejects(gate.waitForDiscovery(new Promise(() => {})), /gate closed before discovery/);
    await gate.close();
    await Promise.all([first, second]);
  } finally {
    await gate.close();
    fixture.cleanup();
  }
});
