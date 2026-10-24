"""Bounded browser integration script. Gates control provider emission only.

No rustX operation, interaction lifetime, or Tool behavior is simulated here.
"""
import json
from fake_provider.scenario import (
    OPENAI_CHAT_COMPLETIONS, Expect, Finish, Gate, HttpError, Raw, Scenario, Step, Stream, Text, ToolCall, Usage,
)


def web_console_dogfood() -> Scenario:
    def expected(prompt: str) -> Expect:
        return Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=(prompt,))

    questions = json.dumps({"questions": [{
        "header": "Direction", "question": "Keep runtime authority in rustX?",
        "options": [{"label": "Keep native (Recommended)", "description": "Use rustX owners."},
                    {"label": "Inspect first", "description": "Read the protocol."}],
        "multi_select": False,
    }]})
    return Scenario(
        "web_console_dogfood",
        Step(expected("Long action in A"), Stream(Text("A is running."), Gate("finish-a"), Text(" A finished."), Finish())),
        Step(expected("Use B while A runs"), Stream(Text("B stayed responsive."), Finish())),
        Step(expected("Approval please"), Stream(ToolCall("console-bash", "bash", json.dumps({
            "command": "printf console-approved; printf x >> console-effect", "execution_mode": "foreground",
        })), Finish("tool_calls"))),
        Step(expected("console-approved"), Stream(Text("Approval completed."), Finish())),
        Step(expected("Questionnaire please"), Stream(ToolCall("console-question", "ask_user", questions), Finish("tool_calls"))),
        Step(expected("Keep native"), Stream(Text("Questionnaire completed."), Finish())),
        Step(expected("Publish while detached"), Stream(Text("Preparing a question."), Gate("publish-question"),
             ToolCall("console-detached-question", "ask_user", questions), Finish("tool_calls"))),
        Step(expected("Keep native"), Stream(Text("Detached question completed."), Finish())),
    )


SCENARIOS = {"web_console_dogfood": web_console_dogfood}


def web_chat_history() -> Scenario:
    """Thirty-four accepted turns cross the native 64-entry bootstrap boundary."""
    steps = [Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=(f"History {i}",)),
                  Stream(Text(f"Answer {i}"), Finish())) for i in range(34)]
    steps.append(Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("Rich reply",)),
                      Stream(Text("## Rich reply\n\n| Key | Value |\n| --- | --- |\n| native | history |\n\n```rust\nfn main() {}"),
                             Gate("settle-chat"), Text("\n```\n\n- **Settled**\n\n$x^2$"), Finish())))
    steps.append(Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("Keep this draft", "user_uploaded_files", "note.txt", ".agents/uploads/")),
                      Stream(Text("Uploaded workspace file received."), Finish())))
    steps.append(Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("Image please",)),
                      Stream(ToolCall("chat-image", "render_image", "{}"), Finish("tool_calls"))))
    steps.append(Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("Image artifact", "chat-image")), Stream(Text("Image reference retained for a vision-capable model."), Finish("stop"))))
    return Scenario("web_chat_history", *steps)


SCENARIOS["web_chat_history"] = web_chat_history


def web_commands() -> Scenario:
    """Real native retry consumes the returned boundary input, never old output."""
    def expected(model: str) -> Expect:
        return Expect(protocol=OPENAI_CHAT_COMPLETIONS, model=model,
                      body_contains=("Regenerate my uploaded note", "user_uploaded_files", "note.txt", ".agents/uploads/"),
                      body_excludes=("Original native answer", "Regenerated native answer"))
    return Scenario("web_commands",
                    Step(expected("second-model"), Stream(Text("Original native answer"), Finish(), Usage(100, 20))),
                    Step(expected("second-model"), Stream(Gate("retry-request-reached"), Text("Regenerated native answer"), Finish())),
                    Step(expected("second-model"), Stream(Gate("inherited-retry-reached"), Text("Inherited replay answer"), Finish())))


SCENARIOS["web_commands"] = web_commands


def web_response_retry() -> Scenario:
    """Keep an earlier turn while a native sibling replay replaces this response."""
    def expected(prompt: str, **kwargs) -> Expect:
        return Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=(prompt,), **kwargs)
    return Scenario("web_response_retry",
                    Step(expected("Earlier question"), Stream(Text("Earlier answer stays."), Finish())),
                    Step(expected("Retry this question"), Stream(Text("## Original response\n\nOld reply to replace."), Finish())),
                    Step(expected("Retry this question", body_excludes=("Original response", "Old reply to replace.")),
                         Stream(Gate("replay-request"), Text("## New response\n\nGenerating the replacement."),
                                Gate("replay-streaming"), Text("\n\nReplacement complete."), Finish())))


SCENARIOS["web_response_retry"] = web_response_retry


def web_sidebar_actions() -> Scenario:
    """Sidebar fork retains a native completion while its source keeps running."""
    def expected(prompt: str, **kwargs) -> Expect:
        return Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=(prompt,), **kwargs)
    return Scenario("web_sidebar_actions",
                    Step(expected("Sidebar source"), Stream(Text("Completed sidebar answer."), Finish())),
                    Step(expected("Keep source running"), Stream(Text("Unfinished source output."), Gate("sidebar-source-running"), Text(" Source finished."), Finish()), allow_disconnect=True),
                    Step(expected("Fork follow-up", body_excludes=("Keep source running", "Unfinished source output.")),
                         Stream(Text("Fork kept the completed answer."), Finish())))


SCENARIOS["web_sidebar_actions"] = web_sidebar_actions


def web_composer_context() -> Scenario:
    """Native Todo and Goal owners feed the composer docks; one Human input waits in the mailbox.

    The provider only emits model calls. The Todo list, Goal state, continuation
    round admission and the pending inbound row all belong to rustX.
    """
    def expected(text: str) -> Expect:
        return Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=(text,))

    def todo(call: str, arguments: dict[str, object]) -> ToolCall:
        return ToolCall(call, "todo", json.dumps(arguments))

    return Scenario(
        "web_composer_context",
        Step(expected("Plan the composer docks"), Stream(
            todo("todo-bind", {"action": "create", "subject": "Bind native Todo"}),
            todo("todo-goal", {"action": "create", "subject": "Render the Goal dock", "blocked_by": [1]}),
            todo("todo-start", {"action": "update", "id": 1, "status": "in_progress", "active_form": "Binding native Todo"}),
            Finish("tool_calls"))),
        Step(expected("Binding native Todo"), Stream(Text("Plan recorded."), Finish())),
        Step(expected("Keep working until the docks are verified"), Stream(ToolCall("goal-create", "create_goal", json.dumps(
            {"objective": "Verify the composer docks", "autonomous_round_budget": 1})), Finish("tool_calls"))),
        Step(expected("Verify the composer docks"), Stream(Text("Goal recorded."), Finish())),
        Step(expected("Continue pursuing the current Goal"), Stream(
            Text("Continuing the Goal."), Gate("goal-round"), Text(" Round finished."), Finish())),
        Step(expected("Queued during the Goal round"), Stream(Text("Queued input handled."), Finish())),
    )


SCENARIOS["web_composer_context"] = web_composer_context


def web_upload_conformance() -> Scenario:
    """The model requests ordinary workspace Tool IO, before/after source deletion.

    The glob only discovers the server-chosen batch name. The browser separately
    asserts the model request's exact absolute path against the upload receipt.
    No ArtifactStore or emulator filesystem reads participate.
    """
    command = "for f in .agents/uploads/*/*/acceptance.txt; do printf '%s\\n' \"$PWD/$f\"; cat \"$f\"; done"
    steps = []
    for phase in ("Source", "Destination"):
        steps.extend([
            Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                        body_contains=("Use my uploaded files", "<user_uploaded_files>", "acceptance.txt")),
                 Stream(ToolCall(f"upload-{phase.lower()}", "bash", json.dumps({
                     "command": command, "execution_mode": "foreground",
                 })), Finish("tool_calls"))),
            Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                        body_contains=("UPLOAD_NATIVE_SENTINEL", ".agents/uploads/")),
                 Stream(Text(f"{phase} upload read through native Tool."), Finish())),
        ])
    return Scenario("web_upload_conformance", *steps)


SCENARIOS["web_upload_conformance"] = web_upload_conformance


def web_workflow_conformance() -> Scenario:
    # Reuse the native conformance script, including its child-admission gate.
    from dataclasses import replace
    from .conformance import workflow_output
    return Scenario("web_workflow_conformance", *(
        replace(step, expect=replace(step.expect, model="console-model"))
        for step in workflow_output().steps
    ))


def web_workflow_observation() -> Scenario:
    # Ordinary child Tool execution, then a separate reserved result commit.
    return Scenario(
        "web_workflow_observation",
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                    body_contains=("workflow conformance request",), tools_include=("review_pr",)),
             Stream(ToolCall("call-review-pr", "review_pr", '{"task":"workflow conformance request"}'), Finish("tool_calls"))),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                    body_contains=("Review the request",), tools_include=("read", "workflow_output")),
             Stream(Gate("workflow-child-admitted"), Text("Inspecting the review profile."),
                    ToolCall("call-child-read", "read", '{"path":".agents/agents/reviewer.toml"}', pieces=2), Finish("tool_calls"))),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                    body_contains=("Workflow-only reviewer", "call-child-read"), tools_include=("workflow_output",)),
             Stream(Gate("workflow-child-read"), ToolCall("call-workflow-output", "workflow_output",
                    '{"passed":true,"summary":"native workflow child committed"}'), Finish("tool_calls"))),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                    body_contains=("native workflow child committed",), body_excludes=("workflow_output",)),
             Stream(Text("workflow conformance complete"), Finish())),
    )

SCENARIOS["web_workflow_observation"] = web_workflow_observation

SCENARIOS["web_workflow_conformance"] = web_workflow_conformance


def web_session_archive() -> Scenario:
    """One settled native history shared by browser and remote TUI exports."""
    return Scenario("web_session_archive", Step(
        Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("Archive this Session",)),
        Stream(Text("Archive fixture settled."), Finish()),
    ))


SCENARIOS["web_session_archive"] = web_session_archive


def web_trace_convergence() -> Scenario:
    """Request input is frozen before the save/adoption gates in the browser suite."""
    return Scenario(
        "web_trace_convergence",
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("Before adoption",)),
             Stream(Text("Frozen before configuration."), Gate("trace-before"), Finish())),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("After adoption", "TRACE_NEW_INSTRUCTIONS")),
             Stream(Text("Frozen after explicit adoption."), Finish())),
    )


SCENARIOS["web_trace_convergence"] = web_trace_convergence


def web_harness_convergence() -> Scenario:
    """Real native Tool and status projection for the Issue #402 browser flow."""
    from fake_provider.scenario import Reasoning
    expected = Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="second-model",
                      body_contains=("Converge the conversation",))
    return Scenario(
        "web_harness_convergence",
        Step(expected, Stream(Reasoning("## Plan\n\n- Inspect the workspace.\n- Keep **native authority**.\n\n`exact identity`"),
                              Text("I will prepare a small workspace change."),
                              ToolCall("write-402", "write", json.dumps({"path": "hello.txt", "content": "hello native world\n"})), Finish("tool_calls"))),
        Step(expected, Stream(Reasoning("Verify the committed file before answering."),
                              ToolCall("bash-402", "bash", json.dumps({"command": "cat hello.txt", "execution_mode": "foreground"})), Finish("tool_calls"))),
        Step(expected, Stream(ToolCall("read-402", "read", json.dumps({"path": "hello.txt"})), Finish("tool_calls"))),
        Step(expected, Stream(Text("## Workspace ready\n\nThe file was written and verified through native Tools."), Finish())),
    )


SCENARIOS["web_harness_convergence"] = web_harness_convergence


def web_agent_continuation() -> Scenario:
    """Hold real native parent/child requests, then interrupt and resume the child.

    Parent continuation and first child request can arrive in either order.
    The browser observes recorded input to release the correct provider gate;
    all Agent identity, cancellation, settlement, and continuation is native.
    """
    return Scenario(
        "web_agent_continuation",
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                    body_contains=("WEB_AGENT_PARENT",), tools_include=("subagent",)),
             Stream(ToolCall("web-agent-create", "subagent", json.dumps({
                 "agent": "reviewer", "title": "Review workspace", "task": "WEB_AGENT_CHILD: review the workspace",
             })), Finish("tool_calls"))),
        *(Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model"),
               Stream(Gate(f"agent-initial-{index}"), Text("Initial request completed."), Finish()),
               allow_disconnect=True) for index in range(2)),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                    body_contains=("WEB_AGENT_RESUME",), body_excludes=("WEB_AGENT_PARENT",)),
             Stream(Gate("agent-resumed"), Text("Resumed canonical child report."), Finish())),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                    body_contains=("WEB_AGENT_PARENT", "Resumed canonical child report.")),
             Stream(Text("Parent received the resumed child report."), Finish())),
    )


SCENARIOS["web_agent_continuation"] = web_agent_continuation


def web_composer_identity() -> Scenario:
    """Two real native Attempts; editor tests hold only the decoded input ACK."""
    return Scenario("web_composer_identity", *[
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                    body_contains=(f"Composer identity {i}",)),
             Stream(Text(f"Canonical composer reply {i}"), Finish()))
        for i in range(2)
    ])


SCENARIOS["web_composer_identity"] = web_composer_identity


def web_reading_surface() -> Scenario:
    """300 real native Attempts exceed 512 transcript entries; gates prove follow."""
    steps = [Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                         body_contains=(f"Reading {i}",)),
                  Stream(Text(f"Native reading answer {i}\n\n" + "Readable history paragraph."), Finish()))
             for i in range(300)]
    steps.append(Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                             body_contains=("Stream reading",)),
                      Stream(Text("Reading stream begins.\n\n"), Gate("reading-detached"),
                             Text("Detached output.\n\n" + "\n\n".join(
                                 f"Measured paragraph {i} retains position at checkpoint {i * i + 17}."
                                 for i in range(60))), Gate("reading-latest"),
                             Text("Following output.\n\n" + "\n\n".join(
                                 f"Live paragraph {i} advances the reading tail through checkpoint {i * i + 29}."
                                 for i in range(60))), Finish())))
    return Scenario("web_reading_surface", *steps)


SCENARIOS["web_reading_surface"] = web_reading_surface


def web_reading_scale_1000() -> Scenario:
    """Independent scaling measurement; the browser's 300-Attempt script is unchanged."""
    return Scenario("web_reading_scale_1000", *[
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                    body_contains=(f"Reading {i}",)),
             Stream(Text(f"Native reading answer {i}\n\n" + "Readable history paragraph."), Finish()))
        for i in range(1000)
    ])


SCENARIOS["web_reading_scale_1000"] = web_reading_scale_1000


def web_file_delivery() -> Scenario:
    """Ordinary Write/Bash/present Tool settlement; the browser owns no file facts."""
    markdown = '# Delivered report\r\n\r\n**Original** Unicode bytes.\r\n\r\n<script>globalThis.PWNED=1</script>\r\n\r\n![blocked](https://example.org/tracker)\r\n'
    script = """import base64,json,pathlib
p=pathlib.Path('.')
(p/'plain 空格.txt').write_bytes(b'plain\\r\\noriginal\\r\\n')
(p/'source.rs').write_bytes(b'fn main() { println!(\"native\"); }\\n')
(p/'pixel.png').write_bytes(base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aD1sAAAAASUVORK5CYII='))
(p/'data.bin').write_bytes(bytes([0,1,255,128,10]))
(p/'large.txt').write_bytes(b'x'*(300*1024))
for name in ('a','b'):
 (p/name).mkdir()
 (p/name/'report.md').write_text('Ambiguous basename')
print(json.dumps({'deliveries':[{'path':'a/report.md'}]}))
"""
    files = [{'path': name, 'description': description} for name, description in [
        ('报告 file.md', 'Explicit Markdown report'), ('plain 空格.txt', 'Created by Bash'),
        ('source.rs', 'Source code'), ('pixel.png', 'Raster image'),
        ('data.bin', 'Unsupported inline format'), ('large.txt', 'Above the managed Artifact threshold'),
    ]]
    expected = Expect(protocol=OPENAI_CHAT_COMPLETIONS, model='console-model', body_contains=('Deliver files explicitly',))
    def tool(call: str, name: str, arguments: dict[str, object]) -> Stream:
        return Stream(ToolCall(call, name, json.dumps(arguments, ensure_ascii=False)), Finish('tool_calls'))
    return Scenario('web_file_delivery',
        Step(expected, tool('delivery-write', 'write', {'path': '报告 file.md', 'content': markdown})),
        Step(expected, tool('delivery-bash', 'bash', {'command': "python3 - <<'PY'\n" + script + 'PY\n', 'execution_mode': 'foreground'})),
        Step(expected, Stream(Text('I created report.md and 报告 file.md.'), Gate('delivery-before-declaration'),
                              ToolCall('delivery-malformed', 'present', json.dumps({'files': [{'path': '../escape'}]})), Finish('tool_calls'))),
        Step(expected, tool('delivery-present', 'present', {'files': files + [{'path': './报告 file.md', 'description': 'Duplicate discarded'}]})),
        Step(expected, tool('delivery-repeat', 'present', {'files': [{'path': '报告 file.md', 'description': 'Repeated explicit declaration'}]})),
        Step(expected, Stream(Text('Delivery finished.'), Finish())),
    )


SCENARIOS['web_file_delivery'] = web_file_delivery


def web_compaction() -> Scenario:
    """Manual compaction, a real questionnaire, then overflow compaction and continuation."""
    expected = Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                      body_contains=("compaction-evidence-435",))
    return Scenario("web_compaction",
                    Step(expected, Stream(Text("Original context ready."), Finish(), Usage(32000, 20))),
                    Step(expected, Stream(Gate("manual-summary"), Text("The user supplied compaction-evidence-435. Preserve that fact."), Finish())),
                    Step(expected, Stream(ToolCall("after-compact-question", "ask_user", json.dumps({"questions": [{
                        "header": "Continuation", "question": "Continue after compaction?",
                        "options": [{"label": "Continue", "description": "Keep the compacted history."},
                                    {"label": "Inspect", "description": "Inspect the history first."}],
                    }]})), Finish("tool_calls"))),
                    Step(expected, HttpError(400, {"error": {"message": "context window exceeded", "type": "invalid_request_error", "code": "context_length_exceeded"}})),
                    Step(expected, Stream(Gate("automatic-summary"), Text("Preserve compaction-evidence-435 and the user's instruction to continue."), Finish())),
                    Step(expected, Stream(Text("Question answered after compaction."), Finish())),
                    Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("Stop a partial reply",)),
                         Stream(Text("## Partial answer\n\nThis text was already released."), Gate("stop-partial"), Text("This must not appear after stopping."), Finish()), allow_disconnect=True),
                    Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("Continue after stopping",)),
                         Stream(Text("Conversation continued after stopping."), Finish())))


SCENARIOS["web_compaction"] = web_compaction


def web_stop() -> Scenario:
    """Provider gates cover real cancellation before/during output and Tools."""
    import hashlib
    from fake_provider.scenario import Reasoning

    long_body = "\n".join(f"Record {index:04d}: {hashlib.sha256(str(index).encode()).hexdigest()}" for index in range(512))

    def expected(prompt: str) -> Expect:
        return Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=(prompt,))

    return Scenario("web_stop",
        Step(expected("Stop before output"), Stream(Gate("stop-empty"), Text("Abandoned empty response."), Finish()), allow_disconnect=True),
        Step(expected("Stop during reasoning"), Stream(Reasoning("Preserved reasoning before Stop."), Gate("stop-reasoning"), Text("Abandoned reasoning response."), Finish()), allow_disconnect=True),
        Step(expected("Stop during text"), Stream(Reasoning("Preserved reasoning beside text."), Text("## Preserved partial answer\n\nAlready delivered text.\n\n```rust\nfn unfinished() {"), Gate("stop-text"), Text("Abandoned text response."), Finish()), allow_disconnect=True),
        Step(expected("Stop during long text"), Stream(Text("## Long partial answer\n\n"), Text(long_body + "\n\nPreserved long tail.", pieces=16), Gate("stop-long"), Text("Abandoned long response."), Finish()), allow_disconnect=True),
        Step(expected("Stop during proposal"), Stream(Text("Preparing a file without executing it."),
            Raw(b'data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"stopped-write","type":"function","function":{"name":"write","arguments":"{\\\"path\\\":"}}]},"finish_reason":null}]}\n\n'),
            Gate("stop-proposal"), Finish("tool_calls")), allow_disconnect=True),
        Step(expected("Stop running tool"), Stream(ToolCall("stopped-bash", "bash", json.dumps({"command": "sleep 30; printf must-not-run", "execution_mode": "foreground"})), Finish("tool_calls"))),
        Step(expected("Continue after all stops"), Stream(Text("The conversation continued after all stops."), Finish())))


SCENARIOS["web_stop"] = web_stop


def web_advanced_documents() -> Scenario:
    expected = Expect(protocol=OPENAI_CHAT_COMPLETIONS, model='console-model', body_contains=('Preview advanced files',))
    files = [{'path': name} for name in ['sample.pdf', 'sample.docx', 'sample.pptx', 'sample.xlsx', 'benign.html', 'hostile.html', 'malformed.pdf', 'encrypted.pdf']]
    return Scenario('web_advanced_documents',
        Step(expected, Stream(ToolCall('documents-present', 'present', json.dumps({'files': files})), Finish('tool_calls'))),
        Step(expected, Stream(Text('Documents delivered.'), Gate('document-stream'), Text(' Streaming continues.'), Finish())),
    )

SCENARIOS['web_advanced_documents'] = web_advanced_documents


def web_preview_workspace() -> Scenario:
    """Committed exact sources followed by gated live conversation growth."""
    expected = Expect(protocol=OPENAI_CHAT_COMPLETIONS, model='console-model', body_contains=('Preview workspace files',))
    files = [{'path': name} for name in ['a/sample.pdf', 'b/sample.pdf', 'sample.docx', 'sample.pptx', 'sample.xlsx', 'benign.html', 'notes.txt', 'second.txt']]
    history = '\n\n'.join(f'Workspace reading paragraph {i}. ' + 'Preserve this detached reading position. ' * 8 for i in range(18))
    return Scenario('web_preview_workspace',
        Step(expected, Stream(Text(history), ToolCall('workspace-present', 'present', json.dumps({'files': files})), Finish('tool_calls'))),
        Step(expected, Stream(Text('Workspace files delivered.'), Gate('workspace-stream'), Text(' New streamed workspace content.'), Finish())),
    )


SCENARIOS['web_preview_workspace'] = web_preview_workspace


SCENARIOS['web_artifact_document'] = lambda: Scenario('web_artifact_document')


def web_model_errors() -> Scenario:
    expected = Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model")
    return Scenario("web_model_errors",
        Step(expected, HttpError(401, {"error": {"message": "Fixture authentication rejected", "type": "authentication_error"}})),
        Step(expected, Stream(Text("Partial output stays readable."), Gate("invalid-response"),
                             Raw(b'data: {"choices":"invalid"}\n\n')), allow_disconnect=True),
        Step(expected, Stream(Text("Conversation recovered after model errors."), Finish())))

SCENARIOS["web_model_errors"] = web_model_errors


def web_timeout_retry() -> Scenario:
    expected = Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model")
    timeout = HttpError(408, {"error": {"message": "Fixture model response timeout", "type": "timeout"}})
    return Scenario("web_timeout_retry",
        Step(expected, timeout),
        Step(expected, Stream(Gate("retry-started"), Text("Recovered after timeout."), Finish())),
        Step(expected, timeout),
        Step(expected, Stream(Gate("stop-retry"), Text("Late retry output must stay hidden."), Finish()), allow_disconnect=True))

SCENARIOS["web_timeout_retry"] = web_timeout_retry

def web_terminal_ownership() -> Scenario:
    """One native message supplies a branch boundary; ownership changes run no model work."""
    return Scenario(
        "web_terminal_ownership",
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                    body_contains=("Ownership branch seed",)),
             Stream(Text("Committed ownership test boundary."), Finish())),
    )


SCENARIOS["web_terminal_ownership"] = web_terminal_ownership


def web_child_capabilities() -> Scenario:
    """Full native child streaming, active/inactive uploads and output deliveries."""
    return Scenario(
        "web_child_capabilities",
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                    body_contains=("CHILD_CAPABILITIES_PARENT",), tools_include=("subagent",)),
             Stream(ToolCall("child-capabilities-create", "subagent", json.dumps({
                 "agent": "reviewer", "title": "Research UI capabilities", "task": "CHILD_CAPABILITIES_TASK",
             })), Finish("tool_calls"))),
        *(Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model"),
               Stream(Text("Live child first chunk."), Gate(f"child-first-{index}"),
                      Text(" Second live chunk."), Gate(f"child-second-{index}"), Finish())) for index in range(2)),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", tools_include=("write",),
                    body_contains=("ACTIVE_CHILD_UPLOAD", "active.txt", "user_uploaded_files", ".agents/uploads/"),
                    body_excludes=("CHILD_CAPABILITIES_PARENT",)),
             Stream(ToolCall("child-write-report", "write", json.dumps({
                 "path": "child-report.md", "content": "# Child deliverable\n\n**Verified native output.**\n",
             })), Finish("tool_calls"))),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("child-write-report",)),
             Stream(ToolCall("child-present-report", "present", json.dumps({"files": [{"path":"child-report.md", "description":"Child result"}]})), Finish("tool_calls"))),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", tools_include=("render_image",)),
             Stream(ToolCall("child-image", "render_image", "{}"), Finish("tool_calls"))),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("child-image",)),
             Stream(Text("Canonical child final report."), Finish(), Usage(400, 40))),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
                    body_contains=("resume.txt", "user_uploaded_files", "CHILD_CAPABILITIES_TASK"),
                    body_excludes=("CHILD_CAPABILITIES_PARENT",)),
             Stream(Text("File-only child continuation completed."), Finish(), Usage(500, 30))),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=("CHILD_CAPABILITIES_PARENT",)),
             Stream(Text("Parent and child remain independent."), Finish())),
    )

SCENARIOS["web_child_capabilities"] = web_child_capabilities


def web_config_repair() -> Scenario:
    """Automatic configuration repair needs no model request at all (#464)."""
    return Scenario("web_config_repair")


SCENARIOS["web_config_repair"] = web_config_repair


def web_fork_source_reading() -> Scenario:
    """Seed distant history; every source navigation remains a read."""
    return Scenario("web_fork_source_reading", *web_reading_surface().steps[:-1])


SCENARIOS["web_fork_source_reading"] = web_fork_source_reading


def web_large_write() -> Scenario:
    """A real write with deferred arguments followed by native file delivery."""
    content = "# Native write report\n\n" + "\n".join(
        f"Row {i}: preserve original content 水 and native tool identity." for i in range(300)
    )
    expected = Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model")
    return Scenario("web_large_write",
        Step(expected, Stream(ToolCall("large-write", "write", json.dumps({
            "path": "large-write.md", "content": content,
        }, ensure_ascii=False)), Finish("tool_calls"))),
        Step(expected, Stream(ToolCall("large-write-present", "present", json.dumps({
            "files": [{"path": "large-write.md", "description": "Native write report"}],
        })), Finish("tool_calls"))),
        Step(expected, Stream(Text("Large write finished."), Finish())),
    )


SCENARIOS["web_large_write"] = web_large_write


def web_workflow_review() -> Scenario:
    from dataclasses import replace
    original = web_workflow_conformance()
    steps = list(original.steps)
    report = {"overview": "## Review report\n\nThe workflow read the request and completed its analysis.",
              "issues": ["Check the release version before publishing."], "candidate": None}
    steps[0] = replace(steps[0], respond=Stream(ToolCall("call-review-pr", "review_pr", json.dumps({"task": "workflow conformance request", "report": report})), Finish("tool_calls")))
    steps[-1] = replace(steps[-1], expect=replace(steps[-1].expect, body_contains=("workflow conformance request",)))
    return Scenario("web_workflow_review", *steps)


SCENARIOS["web_workflow_review"] = web_workflow_review


def web_session_title_primary() -> Scenario:
    """Primary execution is independent of the title provider's physical order."""
    return Scenario("web_session_title_primary", Step(
        Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model",
               body_contains=("Investigate the cache invalidation bug",)),
        Stream(Text("Primary task completed."), Finish(), Usage(40, 8)),
    ))


def web_session_title_auxiliary() -> Scenario:
    """Assert native Title semantics at the real HTTP provider boundary."""
    return Scenario("web_session_title_auxiliary", Step(
        Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="title-model", no_direct_tools=True,
               body_contains=("Investigate the cache invalidation bug", "Write a concise title"),
               json_subset={"max_completion_tokens": 128, "title_tag": "explicit-native"}),
        Stream(Gate("title-dispatched"), Text("Cache invalidation investigation"), Finish(), Usage(30, 5)),
    ))


SCENARIOS["web_session_title_primary"] = web_session_title_primary
SCENARIOS["web_session_title_auxiliary"] = web_session_title_auxiliary


def web_auxiliary() -> Scenario:
    """A real concurrent Agent plus one ephemeral question and one cancellation."""
    def expected(text: str) -> Expect:
        return Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", body_contains=(text,))
    return Scenario("web_auxiliary",
        Step(expected("Primary remains running"), Stream(Text("Primary provisional text."), Gate("primary"), Text(" Primary complete."), Finish())),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", no_direct_tools=True, body_contains=("What has been committed?", "Answer the temporary question"), body_excludes=("Primary provisional text.",)), Stream(Text("Temporary answer: "), Gate("btw"), Text("committed snapshot only."), Finish())),
        Step(Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="console-model", no_direct_tools=True, body_contains=("Cancel this side question",), body_excludes=("Temporary answer:", "Primary provisional text.")), Stream(Text("Provisional to cancel."), Gate("cancel-btw"), Finish()), allow_disconnect=True),
    )


SCENARIOS["web_auxiliary"] = web_auxiliary


def auxiliary_classification_transport() -> Scenario:
    """Real named RPC, concurrent primary work and strict domain acceptance."""
    def expected(text: str, auxiliary: bool = False) -> Expect:
        return Expect(protocol=OPENAI_CHAT_COMPLETIONS, model="integration-model",
                      body_contains=(text,), no_direct_tools=auxiliary)
    return Scenario("auxiliary_classification_transport",
        Step(expected("Primary classification control"), Stream(Text("Primary control"), Gate("primary"), Text(" complete."), Finish())),
        Step(expected("classify valid input", True), Stream(Text('{"label":"yes"}'), Finish())),
        Step(expected("classify invalid input", True), Stream(Text('{"label":"yes","extra":true}'), Finish())),
    )


SCENARIOS["auxiliary_classification_transport"] = auxiliary_classification_transport
