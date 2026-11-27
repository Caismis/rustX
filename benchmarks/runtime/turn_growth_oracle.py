"""Ordered native conversation contract for the turn-growth fixture (stdlib only)."""
import json

PROMPT, RESPONSE = 'turn growth', 'ok'
TOOL_TEXT = 'fixed tool result'
MUTATION_COMMAND = "printf 'in-turn\\n' > growth-modified.txt\nprintf 'created\\n' > turn-created.txt\nrm turn-deleted.txt\ncat fixture.txt"


def tool_call(turn, mutations):
    if mutations and turn == 1:
        return (f'change-{turn:08}', 'bash', {
            'execution_mode': 'foreground',
            'arguments': {'description': 'Verify workspace changes', 'command': MUTATION_COMMAND},
        })
    return f'read-{turn:08}', 'read', {'arguments': {'path': 'fixture.txt'}}


def text_content(value):
    if isinstance(value, str):
        return value
    assert isinstance(value, list), ('expected textual content', value)
    assert all(set(block) == {'type', 'text'} and block['type'] == 'text'
               and isinstance(block['text'], str) for block in value), value
    return ''.join(block['text'] for block in value)


def verify_request(body, index, kind, mutations=False):
    """Check every native message in order, including assistants and correlations.

    Native system instructions remain a separately identified workload input;
    this oracle does not claim equivalence with another product's tool catalog.
    """
    assert kind in ('text', 'tool') and index >= 0
    messages = body['messages']
    assert messages and messages[0]['role'] == 'system', 'missing native system prompt'
    assert isinstance(messages[0]['content'], str) and messages[0]['content']
    actual = messages[1:]
    current = index // 2 if kind == 'tool' else index
    expected = []
    for turn in range(current + 1):
        expected.append(('user', PROMPT))
        if kind == 'tool' and (turn < current or index % 2):
            expected.extend([('call', tool_call(turn, mutations)), ('tool', tool_call(turn, mutations)[0])])
        if turn < current:
            expected.append(('assistant', RESPONSE))
    assert len(actual) == len(expected), ('message count/order', index, len(actual), len(expected))
    for position, (message, (role, value)) in enumerate(zip(actual, expected)):
        assert message['role'] == ('assistant' if role == 'call' else role), ('role/order', index, position)
        calls = message.get('tool_calls')
        if role == 'call':
            call_id, name, arguments = value
            assert message.get('content') in (None, '', []), ('unexpected assistant content', position)
            assert isinstance(calls, list) and len(calls) == 1, ('tool call count', position)
            call = calls[0]
            assert call['type'] == 'function' and call['id'] == call_id, ('tool call identity', position)
            assert call['function']['name'] == name, ('tool name', position)
            assert json.loads(call['function']['arguments']) == arguments, ('tool envelope', position)
        else:
            assert not calls, ('unexpected tool call', position)
            content = text_content(message['content'])
            if role == 'tool':
                assert message['tool_call_id'] == value, ('tool result identity', position)
                assert TOOL_TEXT in content, ('missing successful tool content', position)
            else:
                assert content == value, ('message content', index, position)
    return {'messages': len(messages), 'users': current + 1,
            'tools': index // 2 + index % 2 if kind == 'tool' else 0}
