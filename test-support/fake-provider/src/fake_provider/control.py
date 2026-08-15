"""Scenario run state: observations, gates, failures, and the report.

This module is the deterministic synchronization core. A test never sleeps
and never guesses: it waits on a *named provider-side observation*, and the
wait is a real barrier (an `asyncio.Condition` inside the provider process),
not a polling loop. Timeouts exist only so a deadlocked test terminates.

```text
provider                                  test driver
--------                                  ------------------------------
accept request        -> request_accepted
write headers         -> headers_sent
write chunk 1         -> chunk_flushed
reach Gate("g")       -> gate_reached ...  await observation gate_reached g
                                           (returns only once it happened)
                                           perform the runtime action
                      <- release gate      POST /gates/g/release
write chunk 2         -> chunk_flushed
```
"""

from __future__ import annotations

import asyncio
from dataclasses import dataclass, field
from typing import Any

from fake_provider.scenario import Gate, RecordedRequest, Scenario, Stream, RawResponse

# Observation kinds. These are the provider-side states a test may wait on.
REQUEST_ACCEPTED = "request_accepted"
HEADERS_SENT = "headers_sent"
CHUNK_FLUSHED = "chunk_flushed"
GATE_REACHED = "gate_reached"
CLIENT_DISCONNECTED = "client_disconnected"
RESPONSE_COMPLETED = "response_completed"
SCENARIO_COMPLETED = "scenario_completed"
ASSERTION_FAILED = "assertion_failed"

GATE_PENDING = "pending"
GATE_REACHED_STATE = "reached"
GATE_RELEASED = "released"


@dataclass(frozen=True)
class Observation:
    """One provider-side state transition, in arrival order."""

    seq: int
    kind: str
    name: str | None = None
    request_index: int | None = None
    detail: dict[str, Any] = field(default_factory=dict)

    def to_json(self) -> dict[str, Any]:
        return {
            "seq": self.seq,
            "kind": self.kind,
            "name": self.name,
            "requestIndex": self.request_index,
            "detail": self.detail,
        }


def declared_gates(scenario: Scenario) -> list[str]:
    """Every gate name the scenario script declares, in script order.

    Gate names must be unique within a scenario: a name is the address a
    test releases, so two gates sharing one would make the release
    ambiguous.
    """
    names: list[str] = []
    for step in scenario.steps:
        script = step.respond.script if isinstance(step.respond, (Stream, RawResponse)) else ()
        for item in script:
            if isinstance(item, Gate):
                if item.name in names:
                    raise ValueError(f"duplicate gate name {item.name!r} in {scenario.name}")
                names.append(item.name)
    return names


class ScenarioRun:
    """The mutable state of one scenario execution."""

    def __init__(self, scenario: Scenario) -> None:
        self.scenario = scenario
        self.requests: list[RecordedRequest] = []
        self.observations: list[Observation] = []
        self.failures: list[dict[str, Any]] = []
        self.steps_consumed = 0
        self.gates: dict[str, str] = {name: GATE_PENDING for name in declared_gates(scenario)}
        self._releases: dict[str, asyncio.Event] = {
            name: asyncio.Event() for name in self.gates
        }
        self._condition = asyncio.Condition()
        self.shutdown_requested = asyncio.Event()

    # -- observations -----------------------------------------------------

    async def observe(
        self,
        kind: str,
        *,
        name: str | None = None,
        request_index: int | None = None,
        **detail: Any,
    ) -> Observation:
        """Records one observation and wakes every waiter."""
        async with self._condition:
            observation = Observation(
                seq=len(self.observations),
                kind=kind,
                name=name,
                request_index=request_index,
                detail=detail,
            )
            self.observations.append(observation)
            self._condition.notify_all()
            return observation

    def _matches(self, kind: str, name: str | None, request_index: int | None) -> list[Observation]:
        return [
            observation
            for observation in self.observations
            if observation.kind == kind
            and (name is None or observation.name == name)
            and (request_index is None or observation.request_index == request_index)
        ]

    async def await_observation(
        self,
        kind: str,
        *,
        name: str | None = None,
        request_index: int | None = None,
        count: int = 1,
        timeout: float,
    ) -> Observation | None:
        """Blocks until the `count`-th matching observation exists.

        Returns `None` on timeout. The timeout is deadlock protection only:
        ordering is established by the observation itself, never by how long
        the caller waited.
        """

        def satisfied() -> bool:
            return len(self._matches(kind, name, request_index)) >= count

        try:
            async with self._condition:
                await asyncio.wait_for(self._condition.wait_for(satisfied), timeout)
                return self._matches(kind, name, request_index)[count - 1]
        except TimeoutError:
            return None

    # -- gates ------------------------------------------------------------

    async def reach_gate(self, name: str, request_index: int) -> None:
        """Marks a gate reached and blocks until the control API releases it."""
        if name not in self._releases:  # pragma: no cover - guarded at load
            raise KeyError(name)
        self.gates[name] = GATE_REACHED_STATE
        await self.observe(GATE_REACHED, name=name, request_index=request_index)
        await self._releases[name].wait()

    async def release_gate(self, name: str) -> bool:
        """Releases a gate. Idempotent; `False` when the gate is unknown."""
        event = self._releases.get(name)
        if event is None:
            return False
        self.gates[name] = GATE_RELEASED
        event.set()
        async with self._condition:
            self._condition.notify_all()
        return True

    # -- failures ---------------------------------------------------------

    async def fail(self, detail: str, *, request_index: int | None = None) -> None:
        """Records a scenario assertion failure.

        A failed scenario stays failed: the process exits non-zero even if
        the driver never asks for the state, so a broken conformance
        expectation can never pass silently.
        """
        self.failures.append({"requestIndex": request_index, "detail": detail})
        await self.observe(ASSERTION_FAILED, request_index=request_index, detail=detail)

    # -- reporting --------------------------------------------------------

    @property
    def ok(self) -> bool:
        return not self.failures and self.steps_consumed == len(self.scenario.steps)

    def state(self) -> dict[str, Any]:
        return {
            "scenario": self.scenario.name,
            "stepsTotal": len(self.scenario.steps),
            "stepsConsumed": self.steps_consumed,
            "requestCount": len(self.requests),
            "gates": dict(self.gates),
            "failures": list(self.failures),
            "observationCount": len(self.observations),
            "complete": self.steps_consumed == len(self.scenario.steps),
            "ok": self.ok,
        }

    def report(self) -> dict[str, Any]:
        unconsumed = [
            index for index in range(self.steps_consumed, len(self.scenario.steps))
        ]
        return {
            **self.state(),
            "unconsumedSteps": unconsumed,
            "requests": [request.to_json() for request in self.requests],
            "observations": [observation.to_json() for observation in self.observations],
        }
