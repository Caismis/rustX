#!/usr/bin/env python3
"""Mutation tests for the independently specified DeepSeek request/history oracle."""
import copy
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import MagicMock, patch

spec = importlib.util.spec_from_file_location('deepseek_reference', Path(__file__).with_name('deepseek-reference.py'))
reference = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reference)
compare_spec = importlib.util.spec_from_file_location('deepseek_compare', Path(__file__).with_name('deepseek-compare.py'))
comparison = importlib.util.module_from_spec(compare_spec)
compare_spec.loader.exec_module(comparison)


def fixture():
    workspace = '/fixture/workspace'
    events, messages, requests = [], [], []

    def append(kind, data, surface=False):
        event = {'type': kind, 'seq': len(events), 'time': 1000 + len(events), 'data': data}
        if surface:
            event['surfaceOp'] = 'append'
        events.append(event)

    def message(role, text):
        native = {'role': role, 'content': [{'type': 'text', 'text': text}]}
        append(role + '/message', native if role == 'user' else {'message': native}, True)
        messages.append({'role': role, 'content': text})

    tools = reference.SDK_ENVELOPE['tools']
    header = {'config': {'provider': 'benchmark', 'model': 'chat-model', 'maxTokens': 1024},
              'tools': [item['function'] for item in tools]}
    for turn in range(41):
        if turn == 40:
            before = [{'type': 'session', 'version': 4, 'id': 'benchmark', 'cwd': workspace}] + copy.deepcopy(events)
            append('session/end-seed', {})
        append('turn/start', {'turn': turn + 1})
        if turn == 0:
            message('system', reference.SDK_ENVELOPE['system'])
            message('user', 'benchmark turn 0000' + reference.REMINDER)
            message('user', reference.SDK_ENVELOPE['runtime_context_template'].replace('<WORKSPACE>', workspace))
        else:
            message('user', f'benchmark turn {0 if turn == 40 else turn:04}')
        if turn in (0, 40):
            append('request/header', {'header': header})
        requests.append({'index': 0 if turn == 40 else turn, 'method': 'POST', 'path': '/v1/chat/completions',
                         'protocol': 'openai_chat_completions', 'body': {
                             'model': 'chat-model', 'messages': copy.deepcopy(messages), 'stream': True,
                             'stream_options': {'include_usage': True}, 'max_completion_tokens': 1024, 'tools': tools}})
        message('assistant', 'L' * 32768 if turn == 40 else f'reply {turn:04}')
        append('turn/end', {'turn': turn + 1, 'reason': {'kind': 'completed'}})
    after = [copy.deepcopy(before[0])] + events
    result = {'profile': 'sdk', 'case': 'short-small', 'workspace': workspace, 'history': 8, 'measured': 32,
              'requests': requests[:40], 'verification_requests': requests[40:],
              'persisted_before_reopen': before, 'persisted_after_reopen': after,
              'first_notifications': copy.deepcopy(before[1:]),
              'verification_notifications': copy.deepcopy(after[len(before) + 1:]),
              'provider_report': {'ok': True, 'requests': requests[:40]},
              'verification_provider_report': {'ok': True, 'requests': requests[40:]},
              'first_exit': {'exit_code': 0}, 'verification_exit': {'exit_code': 0},
              'samples': [{'turn': turn, 'measured': turn >= 8, 'accepted_ns': 10, 'settled_ns': 20,
                           'before': {'pid': {}}, 'after': {'pid': {}}} for turn in range(40)]}
    return result


class HistoryOracleTests(unittest.TestCase):
    def setUp(self):
        self.data = fixture()

    def rejected(self):
        with self.assertRaises(AssertionError):
            reference.verify_result(self.data)

    def test_accepts_every_ordered_request_and_native_reopen(self):
        checked = reference.verify_result(self.data)
        self.assertEqual(checked['ordered_requests'], 41)
        self.assertFalse(checked['cross_engine_full_request_equivalence'])

    def test_rejects_omitted_old_history(self):
        self.data['requests'][-1]['body']['messages'].pop(3)
        self.rejected()

    def test_rejects_reordered_old_history(self):
        messages = self.data['requests'][-1]['body']['messages']
        messages[3], messages[5] = messages[5], messages[3]
        self.rejected()

    def test_rejects_changed_output_even_when_log_and_notifications_agree(self):
        for rows in (self.data['persisted_before_reopen'][1:], self.data['persisted_after_reopen'][1:], self.data['first_notifications']):
            event = next(event for event in rows if event['type'] == 'assistant/message')
            event['data']['message']['content'][0]['text'] = 'changed'
        self.rejected()

    def test_rejects_missing_reopened_history(self):
        self.data['verification_requests'][0]['body']['messages'].pop(5)
        self.rejected()

    def test_rejects_rewritten_persisted_prefix(self):
        self.data['persisted_after_reopen'][1]['time'] += 1
        self.rejected()

    def test_rejects_missing_native_resume_marker(self):
        self.data['persisted_after_reopen'][len(self.data['persisted_before_reopen'])]['type'] = 'request/context'
        self.rejected()

    def test_rejects_arbitrary_extra_native_prompt_text(self):
        self.data['requests'][0]['body']['messages'][2]['content'] += '\nUnexpected instructions'
        self.rejected()

    def test_rejects_first_message_boundary_change(self):
        self.data['requests'][0]['body']['messages'][1:2] = [
            {'role': 'user', 'content': 'benchmark turn 0000'}, {'role': 'user', 'content': reference.REMINDER}]
        self.rejected()

    def test_rejects_missing_sdk_notification(self):
        self.data['first_notifications'].pop()
        self.rejected()

    def test_rejects_failed_provider(self):
        self.data['provider_report']['ok'] = False
        self.rejected()

    def test_rejects_failed_runtime_close(self):
        self.data['verification_exit']['exit_code'] = 9
        self.rejected()

    def test_rejects_changed_native_schema(self):
        self.data['requests'][0]['body']['tools'] = []
        self.rejected()

    def test_rejects_missing_measurement(self):
        self.data['samples'].pop()
        self.rejected()


class ComparisonAdmissionTests(unittest.TestCase):
    def setUp(self):
        self.rows = [{'engine': engine, 'accepted': True} for engine in ('rust', 'pi', 'deepseek')]

    def test_accepts_only_complete_successful_rotation(self):
        self.assertEqual(comparison.admit_case(self.rows, 0), self.rows)

    def test_failed_cross_engine_oracle_excludes_all_rotation_metrics(self):
        self.assertEqual(comparison.admit_case(self.rows, 1), [])

    def test_missing_or_failed_product_excludes_rotation(self):
        self.assertEqual(comparison.admit_case(self.rows[:2], 0), [])
        self.rows[-1]['accepted'] = False
        self.assertEqual(comparison.admit_case(self.rows, 0), [])

    def test_duplicate_engine_cannot_replace_missing_product(self):
        self.rows[-1]['engine'] = 'pi'
        self.assertEqual(comparison.admit_case(self.rows, 0), [])


class RuntimeCounterTests(unittest.TestCase):
    def setUp(self):
        self.data = fixture()
        self.data['completion_boundary'] = 'SDK idle, not final fsync'
        self.data['first_exit'].update(close_ns=1000, wait4={'ru_utime': 1, 'ru_stime': 2, 'ru_oublock': 3})
        for sample in self.data['samples']:
            sample.update(before=self.snapshot(100, 100), after=self.snapshot(100, 100))
        self.data['samples'][-1]['after'] = self.snapshot(200, 200)

    @staticmethod
    def snapshot(cpu, writes, pid='1'):
        fields = ['0'] * 20
        fields[0], fields[11], fields[19] = 'S', str(cpu), '10000'
        return {pid: {'stat': f'{pid} (node) ' + ' '.join(fields), 'io': f'write_bytes: {writes}\n',
                      'smaps_rollup': 'Pss: 100 kB\nPss_Anon: 80 kB\n'}}

    def test_retains_valid_contiguous_cpu_window(self):
        summary = comparison.dsh_summary(self.data, 100)
        self.assertEqual(summary['turn_cpu_ms_per_turn'], 1000 / 32)
        self.assertEqual(summary['turn_write_mib_per_turn'], 100 / 32 / 2**20)

    def test_changed_process_set_makes_counters_unavailable(self):
        self.data['samples'][-1]['after'] = self.snapshot(200, 200, pid='2')
        summary = comparison.dsh_summary(self.data, 100)
        self.assertIsNone(summary['turn_cpu_ms_per_turn'])
        self.assertIsNone(summary['turn_write_mib_per_turn'])
        self.assertIn('PID/start-time set changed', summary['unavailable_metrics']['cpu_ms'])

    def test_decreasing_counters_are_not_reported_as_negative_cost(self):
        self.data['samples'][-1]['after'] = self.snapshot(90, 90)
        summary = comparison.dsh_summary(self.data, 100)
        self.assertIsNone(summary['turn_cpu_ms_per_turn'])
        self.assertIsNone(summary['turn_write_mib_per_turn'])


class ProviderStartupTests(unittest.TestCase):
    def test_failed_ready_frame_reaps_launched_provider(self):
        process = MagicMock()
        process.pid = 765432
        process.stdout = io.StringIO('not-json\n')
        process.poll.return_value = None
        with tempfile.TemporaryDirectory() as temporary:
            stem = Path(temporary) / 'sample'
            with patch.object(reference.subprocess, 'Popen', return_value=process), patch.object(reference.os, 'killpg') as kill:
                with self.assertRaises(json.JSONDecodeError):
                    reference.Provider('large', Path(temporary), stem, {})
            kill.assert_called_once_with(process.pid, reference.signal.SIGKILL)
            process.wait.assert_called_once()
            self.assertEqual(stem.with_suffix('.provider.ready').read_text(), 'not-json\n')

if __name__ == '__main__':
    unittest.main()
