"""Bounded offline runtime benchmark workloads. No external service or credentials."""
from fake_provider.scenario import (
    OPENAI_CHAT_COMPLETIONS, Expect, Finish, Gate, Scenario, Step, Stream, Text, ToolCall,
)


def workload(case: str) -> Scenario:
    count = {"short-small": 40, "short-large": 96}.get(case, 1)
    steps = []
    for turn in range(count):
        script = [Text(f"reply {turn:04}"), Finish()]
        if case == "large":
            script = [Text("L" * 32768, pieces=128), Finish()]
        elif case == "stream":
            script = [item for i in range(128) for item in (Text(f"stream-delta-{i:04} "), Gate(f"delta-{i}"))] + [Finish()]
        elif case == "cancel":
            script = [Text("partial"), Gate("cancel"), Text("must not appear"), Finish()]
        elif case == "tool":
            script = [ToolCall("benchmark-read", "read", '{"path":"{workspace}/note.txt"}'), Finish("tool_calls")]
        expected = [f"benchmark turn {turn:04}"]
        if turn:
            expected.append(f"reply {turn-1:04}")
        steps.append(Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="chat-model", body_contains=tuple(expected)),
                          Stream(*script), allow_disconnect=case == "cancel"))
    if case == "tool":
        steps.append(Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="chat-model", body_contains=("deterministic-note-payload-6d41",)),
                          Stream(Text("deterministic-note-payload-6d41"), Finish())))
    return Scenario(f"runtime_benchmark_{case}", *steps)


SCENARIOS = {f"runtime_benchmark_{case}": lambda case=case: workload(case)
             for case in ("short-small", "short-large", "stream", "large", "tool", "cancel")}
