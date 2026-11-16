"""The scenario registry.

Scenarios are Python definitions, not a YAML/JSON mini-language: Python is
already an adequate DSL for "assert this, then emit that", and a bespoke
document format would need its own parser, its own validation, and its own
failure reporting for no gain.

Every scenario is registered by name. `--scenario <name>` selects one, and
`--list` prints the registry.

The one dynamic value a static script legitimately needs is the workspace
root of the rustX runtime under test. The workspace is a fresh temp directory
per test run, and a scripted tool-call argument may carry the ``{workspace}``
placeholder when a scenario wants to exercise an absolute path. ``--workspace``
substitutes the concrete root at build time. A placeholder without a
``--workspace`` value is a startup error, never a silently broken script.
"""

from __future__ import annotations

import dataclasses
import json
from typing import Callable

from fake_provider.scenario import AdaptiveScenario, Scenario, Step, Stream, ToolCall
from fake_provider.scenarios import app_server, conformance, observation, tui, web_console, runtime_benchmark

#: The placeholder of the concrete absolute workspace root of the runtime
#: under test, substituted into scripted tool-call arguments.
WORKSPACE_PLACEHOLDER = "{workspace}"

#: name -> factory. A factory builds a fresh scenario per process, so a
#: scenario definition can never accumulate state across runs.
SCENARIOS: dict[str, Callable[[], Scenario | AdaptiveScenario]] = {
    **conformance.SCENARIOS,
    **app_server.SCENARIOS,
    **runtime_benchmark.SCENARIOS,
    **observation.SCENARIOS,
    **tui.SCENARIOS,
    **web_console.SCENARIOS,
}


def build(name: str, workspace: str | None = None) -> Scenario | AdaptiveScenario:
    """Builds one registered scenario.

    ``workspace`` is the absolute workspace root of the runtime under test;
    it substitutes every ``{workspace}`` placeholder of the scripted
    tool-call arguments.

    Raises:
        SystemExit: when the name is not registered, listing what is, or
            when a static scenario carries the workspace placeholder without a
            workspace value. Adaptive placeholders are validated when selected.
    """
    factory = SCENARIOS.get(name)
    if factory is None:
        raise SystemExit(
            f"unknown scenario {name!r}; registered: {', '.join(sorted(SCENARIOS))}"
        )
    scenario = factory()
    return _compile_calls(_substitute_workspace(scenario, workspace))


def _map_steps(
    scenario: Scenario | AdaptiveScenario, transform: Callable[[Step], Step]
) -> Scenario | AdaptiveScenario:
    """Apply the same wire transformation to static and selected steps."""
    if isinstance(scenario, AdaptiveScenario):
        def select(requests):
            step, final = scenario.select(requests)
            return transform(step), final

        return dataclasses.replace(scenario, select=select)
    return Scenario(scenario.name, *(transform(step) for step in scenario.steps))


def _compile_calls(scenario: Scenario | AdaptiveScenario) -> Scenario | AdaptiveScenario:
    """Compile the scenario business-call DSL into the runtime envelope.

    Codec/Raw framing fixtures keep their literal bytes. The workflow_output
    terminal protocol has its own contract and is not an ordinary tool.
    """
    def compile_step(step: Step) -> Step:
        if not isinstance(step.respond, Stream):
            return step
        chunks = []
        for chunk in step.respond.script:
            if isinstance(chunk, ToolCall) and chunk.name != "workflow_output":
                try:
                    business = json.loads(chunk.arguments)
                except json.JSONDecodeError:
                    chunks.append(chunk)  # Deliberately malformed argument fixture.
                    continue
                if isinstance(business, dict):
                    metadata = {key: business.pop(key) for key in ("execution_mode", "elevation_reason") if key in business}
                    chunk = dataclasses.replace(chunk, arguments=json.dumps({"arguments": business, **metadata}, separators=(",", ":")))
            chunks.append(chunk)
        return dataclasses.replace(step, respond=Stream(*chunks))

    return _map_steps(scenario, compile_step)


def _substitute_workspace(
    scenario: Scenario | AdaptiveScenario, workspace: str | None
) -> Scenario | AdaptiveScenario:
    """Substitute paths now for static steps and upon adaptive selection.

    A missing workspace raises during build for static placeholders, or when
    the adaptive selector returns a step containing a placeholder.
    """
    def substitute_step(step: Step) -> Step:
        if not isinstance(step.respond, Stream):
            return step
        script = []
        for chunk in step.respond.script:
            if isinstance(chunk, ToolCall) and WORKSPACE_PLACEHOLDER in chunk.arguments:
                if workspace is None:
                    raise SystemExit(
                        f"scenario {scenario.name!r} carries the {WORKSPACE_PLACEHOLDER!r} "
                        "placeholder; start the emulator with --workspace <absolute root>"
                    )
                chunk = dataclasses.replace(
                    chunk, arguments=chunk.arguments.replace(WORKSPACE_PLACEHOLDER, workspace)
                )
            script.append(chunk)
        return dataclasses.replace(step, respond=Stream(*script))

    return _map_steps(scenario, substitute_step)


__all__ = ["SCENARIOS", "build"]
