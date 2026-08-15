"""The scenario model: strict ordered scripts of expectation + response.

A scenario is deliberately non-intelligent. It never inspects the *meaning*
of a prompt to decide what to do; it matches request number N against step
number N, asserts what that step declares, and emits the wire response that
step declares.

```text
step 1: assert request 1 -> emit response 1
step 2: assert request 2 -> emit response 2
...
end:    every step consumed, no unexpected request
```
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Mapping, Union

# ---------------------------------------------------------------------------
# Protocols
# ---------------------------------------------------------------------------

OPENAI_CHAT_COMPLETIONS = "openai_chat_completions"
OPENAI_RESPONSES = "openai_responses"
ANTHROPIC_MESSAGES = "anthropic_messages"

PROTOCOLS = (OPENAI_CHAT_COMPLETIONS, OPENAI_RESPONSES, ANTHROPIC_MESSAGES)


# ---------------------------------------------------------------------------
# High-level response script items
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class Text:
    """Assistant text, streamed as one delta per `pieces` split."""

    text: str
    pieces: int = 1


@dataclass(frozen=True)
class Reasoning:
    """Assistant reasoning/thinking content."""

    text: str


@dataclass(frozen=True)
class ToolCall:
    """A provider-emitted tool call.

    The emulator only *requests* the call. It never executes, simulates, or
    validates a rustX tool: what the tool does, and what result comes back,
    belongs entirely to rustX.
    """

    id: str
    name: str
    arguments: str
    pieces: int = 1


@dataclass(frozen=True)
class Usage:
    """Provider usage reporting."""

    input_tokens: int
    output_tokens: int


@dataclass(frozen=True)
class Finish:
    """The provider finish reason terminating the response."""

    reason: str = "stop"


@dataclass(frozen=True)
class Gate:
    """A provider-side synchronization point.

    Encoding stops here: every preceding byte has been written and flushed,
    and nothing after it is written until the control API releases the gate.
    A test therefore knows exactly what the provider has and has not yet
    sent when it performs its runtime action.
    """

    name: str


@dataclass(frozen=True)
class Raw:
    """Raw bytes written to the wire verbatim.

    The escape hatch for malformed-framing and compatibility scenarios; the
    protocol codec does not touch these bytes.
    """

    data: bytes


@dataclass(frozen=True)
class Disconnect:
    """Close the connection abruptly at this point in the stream."""


Chunk = Union[Text, Reasoning, ToolCall, Usage, Finish, Gate, Raw, Disconnect]


# ---------------------------------------------------------------------------
# Responses
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class Stream:
    """A streamed provider response encoded by the protocol codec."""

    script: tuple[Chunk, ...]

    def __init__(self, *script: Chunk) -> None:
        object.__setattr__(self, "script", tuple(script))


@dataclass(frozen=True)
class HttpError:
    """A provider HTTP error response with a JSON body."""

    status: int
    body: Mapping[str, Any]


@dataclass(frozen=True)
class RawResponse:
    """A fully raw response: the codec encodes nothing.

    Used for deliberately malformed framing and for non-SSE bodies.
    """

    status: int = 200
    headers: Mapping[str, str] = field(default_factory=dict)
    script: tuple[Chunk, ...] = ()


Response = Union[Stream, HttpError, RawResponse]


# ---------------------------------------------------------------------------
# Request expectations
# ---------------------------------------------------------------------------


def _json_subset_failures(expected: Any, actual: Any, path: str) -> list[str]:
    """Recursive subset match: every expected key/value must be present.

    Lists are compared element-wise and must have equal length; scalars must
    be equal. Only *objects* are treated as open, which is what keeps a
    subset assertion from over-coupling a test to unrelated request fields.
    """
    if isinstance(expected, Mapping):
        if not isinstance(actual, Mapping):
            return [f"{path}: expected an object, found {type(actual).__name__}"]
        failures: list[str] = []
        for key, value in expected.items():
            if key not in actual:
                failures.extend([f"{path}.{key}: missing"])
                continue
            failures.extend(_json_subset_failures(value, actual[key], f"{path}.{key}"))
        return failures
    if isinstance(expected, (list, tuple)):
        if not isinstance(actual, list):
            return [f"{path}: expected an array, found {type(actual).__name__}"]
        if len(expected) != len(actual):
            return [f"{path}: expected {len(expected)} elements, found {len(actual)}"]
        failures = []
        for index, value in enumerate(expected):
            failures.extend(_json_subset_failures(value, actual[index], f"{path}[{index}]"))
        return failures
    if expected != actual:
        return [f"{path}: expected {expected!r}, found {actual!r}"]
    return []


@dataclass(frozen=True)
class Expect:
    """What the provider request of one step must look like.

    Everything is optional except the protocol, which selects both the
    served path and the wire codec. Assertions are composable primitives
    rather than a general matcher language: a subset match for structure, a
    substring match for rendered text, and named checks for the few request
    features rustX conformance actually cares about.
    """

    protocol: str
    model: str | None = None
    path: str | None = None
    json_subset: Mapping[str, Any] | None = None
    json_exact: Mapping[str, Any] | None = None
    body_contains: tuple[str, ...] = ()
    body_excludes: tuple[str, ...] = ()
    tools_include: tuple[str, ...] = ()
    no_tools: bool = False
    headers_present: tuple[str, ...] = ()

    def failures(self, request: "RecordedRequest", codec: Any) -> list[str]:
        """Every way this request violates the expectation, in order."""
        failures: list[str] = []
        expected_path = self.path or codec.default_path
        if request.path != expected_path:
            failures.append(f"path: expected {expected_path!r}, found {request.path!r}")
        if request.method != "POST":
            failures.append(f"method: expected 'POST', found {request.method!r}")
        if request.json is None:
            failures.append("body: expected a JSON object")
            return failures
        if self.model is not None:
            found = codec.model_of(request.json)
            if found != self.model:
                failures.append(f"model: expected {self.model!r}, found {found!r}")
        if self.json_exact is not None:
            failures.extend(_json_subset_failures(self.json_exact, request.json, "$"))
            extra = sorted(set(request.json) - set(self.json_exact))
            if extra:
                failures.append(f"$: unexpected top-level keys {extra}")
        if self.json_subset is not None:
            failures.extend(_json_subset_failures(self.json_subset, request.json, "$"))
        for needle in self.body_contains:
            if needle not in request.body_text:
                failures.append(f"body: expected to contain {needle!r}")
        for needle in self.body_excludes:
            if needle in request.body_text:
                failures.append(f"body: expected not to contain {needle!r}")
        if self.tools_include:
            names = codec.tool_names(request.json)
            for name in self.tools_include:
                if name not in names:
                    failures.append(f"tools: expected {name!r}, found {sorted(names)}")
        if self.no_tools and codec.tool_names(request.json):
            failures.append("tools: expected no tool definitions on this request")
        for header in self.headers_present:
            if header.lower() not in request.headers:
                failures.append(f"headers: expected {header!r}")
        return failures


# ---------------------------------------------------------------------------
# Steps and scenarios
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class Step:
    """One request/response pair of a scenario script."""

    expect: Expect
    respond: Response
    #: A step whose stream the test is expected to abandon (cancellation).
    #: Without this, a client disconnect mid-response is a scenario failure.
    allow_disconnect: bool = False


@dataclass(frozen=True)
class Scenario:
    """A strict ordered script."""

    name: str
    steps: tuple[Step, ...]

    def __init__(self, name: str, *steps: Step) -> None:
        object.__setattr__(self, "name", name)
        object.__setattr__(self, "steps", tuple(steps))


# ---------------------------------------------------------------------------
# Request observation
# ---------------------------------------------------------------------------

#: Request headers worth recording. `authorization`, `x-api-key`, and any
#: other credential-bearing header is deliberately absent: the harness
#: records that a credential header *arrived*, never its value.
RECORDED_HEADERS = ("content-type", "accept", "anthropic-version", "user-agent")

CREDENTIAL_HEADERS = ("authorization", "x-api-key", "api-key", "proxy-authorization")


@dataclass
class RecordedRequest:
    """One provider request, in arrival order."""

    index: int
    method: str
    path: str
    protocol: str | None
    model: str | None
    headers: dict[str, str]
    credential_headers: list[str]
    body_text: str
    json: dict[str, Any] | None

    def to_json(self) -> dict[str, Any]:
        return {
            "index": self.index,
            "method": self.method,
            "path": self.path,
            "protocol": self.protocol,
            "model": self.model,
            "headers": self.headers,
            "credentialHeaders": self.credential_headers,
            "body": self.json if self.json is not None else self.body_text,
        }


def sanitize_headers(headers: Mapping[str, str]) -> tuple[dict[str, str], list[str]]:
    """Splits headers into recorded values and present-credential names.

    No credential value is ever retained, logged, or served by the control
    API; only the fact that the header arrived.
    """
    recorded = {
        name: value for name, value in headers.items() if name in RECORDED_HEADERS
    }
    credentials = sorted(name for name in headers if name in CREDENTIAL_HEADERS)
    return recorded, credentials


def split_text(text: str, pieces: int) -> list[str]:
    """Splits text into `pieces` deterministic slices.

    Protocol codecs use this to turn one high-level `Text`/`ToolCall` into
    several wire deltas, which is how a scenario asks for a multi-chunk
    stream without spelling out frames.
    """
    if pieces <= 1 or not text:
        return [text]
    size = max(1, -(-len(text) // pieces))
    return [text[start : start + size] for start in range(0, len(text), size)]
