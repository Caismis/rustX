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
from typing import Callable

from fake_provider.scenario import Scenario, Step, Stream, ToolCall
from fake_provider.scenarios import conformance, tui

#: The placeholder of the concrete absolute workspace root of the runtime
#: under test, substituted into scripted tool-call arguments.
WORKSPACE_PLACEHOLDER = "{workspace}"

#: name -> factory. A factory builds a fresh scenario per process, so a
#: scenario definition can never accumulate state across runs.
SCENARIOS: dict[str, Callable[[], Scenario]] = {
    **conformance.SCENARIOS,
    **tui.SCENARIOS,
}


def build(name: str, workspace: str | None = None) -> Scenario:
    """Builds one registered scenario.

    ``workspace`` is the absolute workspace root of the runtime under test;
    it substitutes every ``{workspace}`` placeholder of the scripted
    tool-call arguments.

    Raises:
        SystemExit: when the name is not registered, listing what is, or
            when the scenario carries the workspace placeholder without a
            workspace value.
    """
    factory = SCENARIOS.get(name)
    if factory is None:
        raise SystemExit(
            f"unknown scenario {name!r}; registered: {', '.join(sorted(SCENARIOS))}"
        )
    scenario = factory()
    return _substitute_workspace(scenario, workspace)


def _substitute_workspace(scenario: Scenario, workspace: str | None) -> Scenario:
    """Substitutes the workspace placeholder of every scripted tool call."""
    steps: list[Step] = []
    changed = False
    for step in scenario.steps:
        if not isinstance(step.respond, Stream):
            steps.append(step)
            continue
        script = tuple(
            dataclasses.replace(
                chunk,
                arguments=chunk.arguments.replace(WORKSPACE_PLACEHOLDER, workspace or ""),
            )
            if isinstance(chunk, ToolCall) and WORKSPACE_PLACEHOLDER in chunk.arguments
            else chunk
            for chunk in step.respond.script
        )
        if script != step.respond.script:
            changed = True
            # Stream's custom __init__ takes the script positionally, so it
            # cannot go through dataclasses.replace.
            steps.append(dataclasses.replace(step, respond=Stream(*script)))
        else:
            steps.append(step)
    if not changed:
        return scenario
    if workspace is None:
        raise SystemExit(
            f"scenario {scenario.name!r} carries the {WORKSPACE_PLACEHOLDER!r} "
            "placeholder; start the emulator with --workspace <absolute root>"
        )
    # Scenario's custom __init__ takes the steps positionally too.
    return Scenario(scenario.name, *steps)


__all__ = ["SCENARIOS", "build"]
