"""Finite capacity fixture using the existing provider's wire and control engine."""
import argparse
import asyncio
import json
import sys

from fake_provider.control import ScenarioRun
from fake_provider.scenario import (
    OPENAI_CHAT_COMPLETIONS, Expect, Finish, Gate, Scenario, Step, Stream, Text,
)
from fake_provider.server import ProviderServer


async def main():
    p = argparse.ArgumentParser()
    p.add_argument('--seed-requests', type=int, required=True)
    p.add_argument('--widths', required=True)
    p.add_argument('--rounds', type=int, required=True)
    a = p.parse_args()
    expect = Expect(protocol=OPENAI_CHAT_COMPLETIONS, model='capacity-model',
                    body_contains=('capacity turn',))
    steps = [Step(expect, Stream(Text('capacity response'), Finish()))
             for _ in range(a.seed_requests)]
    for width in map(int, a.widths.split(',')):
        for turn in range(a.rounds):
            for i in range(width):
                steps.append(Step(expect, Stream(Gate(f'active-{width}-{turn}-{i}'),
                                                Text('capacity response'), Finish())))
    steps.append(Step(expect, Stream(Gate('browse'), Text('capacity response'), Finish())))
    steps.append(Step(expect, Stream(Gate('slow'), Text('S' * 32768, pieces=128), Finish())))
    steps.append(Step(expect, Stream(Text('partial'), Gate('cancel'), Text('forbidden'), Finish()),
                      allow_disconnect=True))
    state = ScenarioRun(Scenario('app_server_capacity', *steps))
    server = ProviderServer(state)
    host, port = await server.start('127.0.0.1', 0)
    print(json.dumps({'ready': True, 'host': host, 'port': port, 'steps': len(steps)}), flush=True)
    await server.serve_until_shutdown()
    report = state.report()
    print(json.dumps({'report': report}), flush=True)
    return 0 if report['ok'] else 1


if __name__ == '__main__':
    sys.exit(asyncio.run(main()))
