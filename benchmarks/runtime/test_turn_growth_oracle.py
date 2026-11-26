"""Adversarial request-history checks, without a provider or timing workload."""
import copy
import json
import unittest

from turn_growth_oracle import PERMISSION_REMINDER, tool_call, verify_request


def text(role, value):
    return {'role': role, 'content': [{'type': 'text', 'text': value}]}


def fixture(tool=False):
    messages = [text('system', 'native prompt'), text('user', 'turn growth'), text('user', PERMISSION_REMINDER)]
    messages[0]['content'] = 'native prompt'
    if tool:
        messages.extend([
            {'role': 'assistant', 'content': [], 'tool_calls': [{
                'id': 'read-00000000', 'type': 'function', 'function': {
                    'name': 'read', 'arguments': '{"arguments":{"path":"fixture.txt"}}'}}]},
            {'role': 'tool', 'tool_call_id': 'read-00000000', 'content': 'fixed tool result\n'},
        ])
    messages.extend([text('assistant', 'ok'), text('user', 'turn growth')])
    return {'messages': messages}


class TurnGrowthOracleTests(unittest.TestCase):
    def test_complete_text_and_tool_history(self):
        self.assertEqual(verify_request(fixture(), 1, 'text')['users'], 2)
        self.assertEqual(verify_request(fixture(True), 2, 'tool')['tools'], 1)

    def test_current_strict_envelopes(self):
        self.assertEqual(tool_call(0, False), ('read-00000000', 'read', {'arguments': {'path': 'fixture.txt'}}))
        call_id, name, envelope = tool_call(1, True)
        self.assertEqual((call_id, name), ('change-00000001', 'bash'))
        self.assertEqual(set(envelope), {'execution_mode', 'arguments'})
        self.assertEqual(envelope['execution_mode'], 'foreground')
        self.assertEqual(set(envelope['arguments']), {'description', 'command'})

    def test_missing_old_assistant_is_rejected(self):
        body = fixture(True)
        del body['messages'][-2]
        with self.assertRaises(AssertionError):
            verify_request(body, 2, 'tool')

    def test_changed_old_assistant_is_rejected(self):
        body = fixture()
        body['messages'][-2]['content'][0]['text'] = 'wrong'
        with self.assertRaises(AssertionError):
            verify_request(body, 1, 'text')

    def test_changed_call_or_result_id_is_rejected(self):
        for position in (3, 4):
            body = fixture(True)
            if position == 3:
                body['messages'][3]['tool_calls'][0]['id'] = 'wrong'
            else:
                body['messages'][4]['tool_call_id'] = 'wrong'
            with self.subTest(position=position), self.assertRaises(AssertionError):
                verify_request(body, 2, 'tool')

    def test_reordered_history_is_rejected(self):
        body = fixture(True)
        body['messages'][3], body['messages'][4] = body['messages'][4], body['messages'][3]
        with self.assertRaises(AssertionError):
            verify_request(body, 2, 'tool')

    def test_flat_or_wrong_business_arguments_are_rejected(self):
        for arguments in ({'path': 'fixture.txt'}, {'arguments': {'path': 'other.txt'}}):
            body = fixture(True)
            body['messages'][3]['tool_calls'][0]['function']['arguments'] = json.dumps(arguments)
            with self.subTest(arguments=arguments), self.assertRaises(AssertionError):
                verify_request(body, 2, 'tool')

    def test_tool_preflight_failure_is_rejected(self):
        body = fixture(True)
        body['messages'][4]['content'] = 'Invalid invocation envelope'
        with self.assertRaises(AssertionError):
            verify_request(body, 2, 'tool')

    def test_extra_system_message_is_not_dropped(self):
        body = fixture()
        body['messages'].insert(2, text('system', 'injected'))
        with self.assertRaises(AssertionError):
            verify_request(body, 1, 'text')

    def test_verification_does_not_mutate_request(self):
        body = fixture(True)
        before = copy.deepcopy(body)
        verify_request(body, 2, 'tool')
        self.assertEqual(body, before)


if __name__ == '__main__':
    unittest.main()
