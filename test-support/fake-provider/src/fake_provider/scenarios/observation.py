"""Issue #467 real client observation while native execution remains gated."""
import hashlib
from fake_provider.scenario import (
    OPENAI_CHAT_COMPLETIONS, Expect, Finish, Gate, Scenario, Step, Stream, Text, Usage,
)


def bounded_observation() -> Scenario:
    return Scenario(
        "bounded_observation",
        Step(
            Expect(
                protocol=OPENAI_CHAT_COMPLETIONS,
                model="integration-model",
                json_subset={"stream": True, "temperature": 0.25},
                body_contains=("hello from the tui",),
                headers_present=("content-type",),
            ),
            Stream(Gate("observation-execution"), Text("".join(f"{index:05d}:{hashlib.sha256(str(index).encode()).hexdigest()} 水🙂\n" for index in range(1_200)), pieces=16), Finish("stop"), Usage(12, 3)),
        ),
    )


SCENARIOS = {"bounded_observation": bounded_observation}
