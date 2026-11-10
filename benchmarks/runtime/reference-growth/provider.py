#!/usr/bin/env python3
"""The same wire workload as Agent A's turn-growth.py, with context assertions."""
import argparse
import asyncio
import hashlib
import json
from pathlib import Path
import sys

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / 'test-support/fake-provider/src'))
from fake_provider.control import ScenarioRun
from fake_provider.scenario import (OPENAI_CHAT_COMPLETIONS, Expect, Finish, Scenario,
                                    Step, Stream, Text, ToolCall, Usage)
from fake_provider.server import ProviderServer
from fake_provider.protocols import codec_for

PROMPT, RESPONSE = 'turn growth', 'ok'


async def main(a):
    expect = Expect(protocol=OPENAI_CHAT_COMPLETIONS, model='turn-growth', body_contains=(PROMPT,))
    steps = []
    for i in range(a.turns):
        if a.scenario == 'tool':
            steps.append(Step(expect, Stream(ToolCall(f'read-{i:08}', 'read', '{"path":"fixture.txt"}'),
                                            Finish('tool_calls'), Usage(16, 1))))
        steps.append(Step(expect, Stream(Text(RESPONSE), Finish(), Usage(16, 1))))
    state = ScenarioRun(Scenario('reference_growth_' + a.scenario, *steps))
    server = ProviderServer(state)
    _, port = await server.start('127.0.0.1', 0)
    print(json.dumps({'port': port, 'steps': len(steps)}), flush=True)
    await server.serve_until_shutdown()
    # Provider CPU, resident request bodies and report construction are excluded.
    report = state.report()
    requests = []
    for i, request in enumerate(state.requests):
        body = request.json
        messages = [m for m in body['messages'] if m['role'] not in ('system', 'developer')]
        users = [m for m in messages if m['role'] == 'user']
        turn = i // (2 if a.scenario == 'tool' else 1)
        assert len(users) == turn + 1, (i, len(users))
        assert all(m['content'] in (PROMPT, [{'type': 'text', 'text': PROMPT}]) for m in users)
        assistants = [m for m in messages if m['role'] == 'assistant' and not m.get('tool_calls')]
        assert len(assistants) == turn, (i, len(assistants), turn)
        assert all(m['content'] in (RESPONSE, [{'type': 'text', 'text': RESPONSE}]) for m in assistants)
        tools = [m for m in messages if m['role'] == 'tool']
        expected_tools = turn + (i % 2) if a.scenario == 'tool' else 0
        assert len(tools) == expected_tools, (i, len(tools), expected_tools)
        assert all(m['content'] in ('fixed tool result\n', [{'type': 'text', 'text': 'fixed tool result\n'}]) for m in tools)
        raw = request.body_text.encode()
        requests.append({'index': i, 'body_bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest(),
                         'messages': len(body['messages']), 'users': len(users), 'tools': len(tools),
                         'tool_names': [t['function']['name'] for t in body.get('tools', [])]})
    # Save canonical SSE bytes/hashes to establish identical provider output.
    codec = codec_for(OPENAI_CHAT_COMPLETIONS)
    streams = []
    for step in steps[:2 if a.scenario == 'tool' else 1]:
        emits = codec.encode(step.respond.script, 'turn-growth')
        chunks = [emit for emit in emits if isinstance(emit, bytes)]
        streams.append([{'bytes': len(chunk), 'sha256': hashlib.sha256(chunk).hexdigest()} for chunk in chunks])
    a.output.write_text(json.dumps({'ok': report['ok'], 'requestCount': report['requestCount'],
                                  'failures': report['failures'], 'requests': requests, 'wire_chunks': streams}, indent=2) + '\n')
    return 0 if report['ok'] else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--turns', type=int, required=True)
    parser.add_argument('--scenario', choices=['text', 'tool'], required=True)
    parser.add_argument('--output', type=Path, required=True)
    sys.exit(asyncio.run(main(parser.parse_args())))
