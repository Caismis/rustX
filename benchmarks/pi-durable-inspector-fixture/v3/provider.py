"""Extra inspector fixture over the unchanged shared fake HTTP/SSE provider."""
from fake_provider.scenarios import SCENARIOS
from fake_provider.scenarios.runtime_benchmark import workload
from fake_provider.scenario import Scenario, Step, Expect, Stream, Text, Gate, Raw, Finish, OPENAI_CHAT_COMPLETIONS
from fake_provider.__main__ import main

def held_detail_stream():
    first=workload('tool').steps[0]
    # Real SSE comments keep the ordinary 15-second stream-idle policy alive
    # while the controller exercises native replay retention. No timeout override.
    script=[]
    for i in range(512):
        if i % 4 == 0:
            script.append(Text(f'inspector-delta-{i//4:04} '))
        script.extend((Raw(b': inspector-liveness\n\n'),Gate(f'inspector-{i}')))
    script.append(Finish())
    return Scenario('runtime_benchmark_tool',first,Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS,model='chat-model',body_contains=('deterministic-note-payload-6d41',)),Stream(*script)))

SCENARIOS['runtime_benchmark_tool']=held_detail_stream
raise SystemExit(main())
