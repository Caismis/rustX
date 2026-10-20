#!/usr/bin/env python3
"""Own product and local HTTP/SSE provider as siblings; retain every failed sample."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import signal
import traceback
import time
import urllib.request
from observer_oracle import validate_pi_versions, validate_rust_settlements

TEST = 'agent_loop::runtime_benchmark::measure_runtime'
OBSERVER_MODES = ['native-workflow', 'no-live-subscriber', 'live-subscriber']
MODEL_TIME = '2026-10-19 00:00:00'
MODEL_REMINDER = '<system-reminder>\nTimezone: UTC\nCurrent time: ' + MODEL_TIME + '\n</system-reminder>'
CASES = ['short-small', 'short-large', 'stream', 'large', 'tool', 'cancel']


def control(base, path):
    with urllib.request.urlopen(urllib.request.Request(base + path, data=b'', method='POST'), timeout=30) as r:
        return json.load(r)


RUST_PHASES = ['before_initialization', 'after_provider_start', 'after_initialization',
               'after_turns', 'after_verification', 'after_shutdown', 'after_driver_drop',
               'after_final_writer_close', 'after_reopen_verification', 'after_cleanup']
PI_PHASES = [p for p in RUST_PHASES if p not in ('after_shutdown', 'after_driver_drop')]


def validate_lifecycle(sample):
    phases = sample['lifecycle']
    expected = list(RUST_PHASES if 'measurement_version' in sample else PI_PHASES)
    if sample.get('observer_mode') in OBSERVER_MODES[1:]:
        expected.insert(expected.index('after_turns') + 1, 'after_late_snapshot')
    assert [point['phase'] for point in phases] == expected, 'missing, duplicate or reordered lifecycle endpoint'
    elapsed = [point['elapsed_us'] for point in phases]
    assert all(isinstance(value, int) and value >= 0 for value in elapsed) and elapsed == sorted(elapsed)
    assert all(set(point['process']) == {'stat', 'io', 'smaps_rollup'} for point in phases)
    return {point['phase']: point for point in phases}


def validate_sample(result, sample, case, log_text):
    assert result['product_exit'] == result['provider_exit'] == 0
    assert result['provider_report']['ok'] is True
    assert sample['oracle'] == 'passed'
    assert sample['workload_version'] == 3 and sample['model_time'] == MODEL_TIME
    mode = result.get('observer_mode', 'native-workflow')
    assert mode in OBSERVER_MODES
    if mode != 'native-workflow' or 'observer_mode' in sample:
        assert sample.get('observer_mode') == mode, 'observer mode missing or mismatched'
        assert all(point.get('observer_mode') == mode for point in sample['samples'])
    expected_requests = {'short-small':40,'short-large':96,'tool':2}.get(case,1)
    assert result['provider_report']['requestCount'] == expected_requests
    wire = sample.get('wire_requests', sample.get('requests'))
    assert isinstance(wire,list) and len(wire) == expected_requests
    assert wire == result['provider_report']['requests']
    for request in wire:
        content = request['body']['messages'][1]['content']
        assert content == MODEL_REMINDER or content == [{'type': 'text', 'text': MODEL_REMINDER}]
    expected_history={'short-small':8,'short-large':64}.get(case,0)
    expected_measured=32 if expected_history else 1
    assert sample['history']==expected_history and sample['measured']==expected_measured
    assert len(sample['samples'])==expected_history+expected_measured
    for i,point in enumerate(sample['samples']):
        assert point['turn']==i and point['measured']==(i>=expected_history)
        assert isinstance(point['settled_us'],int) and point['settled_us']>=0
        assert isinstance(point['accepted_us'],int) and point['accepted_us']>=0
        assert set(point['process'])=={'stat','io','smaps_rollup'}
    validate_lifecycle(sample)
    if 'measurement_version' in sample:
        assert sample['measurement_version'] == (4 if mode != 'native-workflow' else 3)
        assert sample['verified_live_requests'] == sample['verified_reopened_requests'] == expected_requests
        assert len(sample['live_requests']) == len(sample['reopened_requests']) == expected_requests
        assert sample['live_requests'] == sample['reopened_requests']
        assert f'test {TEST} ... ok' in log_text
    else:
        assert len(sample['captures']) == len(sample['reopenedRequests']) == expected_requests
        assert [c['messages'] for c in sample['captures']] == sample['reopenedRequests']
    if mode != 'native-workflow':
        if 'measurement_version' in sample:
            late=sample['late_snapshot']; checkpoint=late['checkpoint']; replay=late['replay']
            expected_messages=copy.deepcopy(sample['canonical'])
            for message in expected_messages:
                if message['role']=='tool':
                    # Declared lossless structural-JSON representation of native u64.
                    message['result']['duration_ms']=str(message['result']['duration_ms'])
            assert late['messages']==expected_messages, 'late snapshot lost or changed ordered messages'
            assert checkpoint['read_cut']['source_cut']['ledger_through']==len(sample['canonical'])
            assert replay['kind'] in ('contiguous','resync_required')
            assert replay.get('through',replay.get('snapshot'))==checkpoint['cursor']
            if mode=='live-subscriber':
                validate_rust_settlements(sample, case)
                observed=sample['observer_validation']
                assert observed['completed_attempts']==expected_history+expected_measured
                expected_text={(m['id'],f'ContentBlockIndex({i})'):b['text']
                    for m in sample['canonical'] if m['role']=='assistant'
                    for i,b in enumerate(m['content']) if b['type']=='text'}
                actual_text={(v['message_id'],v['block_index']):v['text'] for v in observed['text']}
                assert len(actual_text)==len(observed['text']), 'duplicate observed block'
                if case=='cancel' and actual_text:
                    # Cancellation may expose an uncommitted native partial. It is
                    # checked separately, never erased or treated as canonical output.
                    provisional=sample['live_requests'][0]['snapshot']['provisional_message_id']
                    expected_text={(provisional,'ContentBlockIndex(0)'):'partial'}
                assert actual_text==expected_text, 'observed text differs from canonical/provider partial'
                assert observed['tool_results']==[m['result'] for m in sample['canonical'] if m['role']=='tool']
        else:
            assert sample['lateSnapshot']['entries']==sample['entries'], 'late Pi snapshot differs from native entries'
            if mode=='live-subscriber':
                validate_pi_versions(sample, case, result.get('workspace'))
                if case=='cancel':
                    assert 'partial'.startswith(sample['observed_partial_text']), 'cancelled provider tail was published'
    assert sample['lifecycle'][-1]['phase'] == 'after_cleanup'


def run(argv, stem, case, env, project):
    began = time.monotonic_ns()
    product = provider = None
    result = {'argv': argv, 'case': case, 'accepted': False,
              'observer_mode': env.get('RUSTX_BENCH_OBSERVER_MODE', 'native-workflow')}
    try:
        with stem.with_suffix('.log').open('w') as log, stem.with_suffix('.provider.log').open('w') as provider_log:
            product = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env, start_new_session=True)
            for line in product.stdout:
                log.write(line)
                log.flush()
                if line.startswith('RUSTX_BENCH_READY '):
                    ready = json.loads(line.split(' ', 1)[1])
                    break
            else:
                raise RuntimeError('product exited before provider handshake')
            result['product_startup_ns'] = time.monotonic_ns() - began
            result['workspace'] = ready['workspace']
            provider_start = time.monotonic_ns()
            provider = subprocess.Popen(['uv', 'run', '--project', str(project), '--frozen', 'fake-provider', '--scenario', ready['scenario'], '--workspace', ready['workspace'], '--port', '0'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=provider_log, text=True, env=env, start_new_session=True)
            provider_ready = json.loads(provider.stdout.readline())
            assert provider_ready['ready'] is True and provider_ready['host'] == '127.0.0.1'
            result['provider_startup_ns'] = time.monotonic_ns() - provider_start
            product.stdin.write(json.dumps(provider_ready) + '\n')
            product.stdin.flush()
            for line in product.stdout:
                log.write(line)
            log.flush()
            result['product_exit'] = product.wait()
            result['through_product_exit_ns'] = time.monotonic_ns() - began
            # Keep the provider alive until the product has closed/reopened/cleaned
            # up and exited; no product counter can contain provider reaping.
            teardown = time.monotonic_ns()
            result['provider_report'] = control(f"http://127.0.0.1:{provider_ready['port']}", '/__control/shutdown')
            stem.with_suffix('.provider.stdout').write_text(provider.stdout.read())
            result['provider_exit'] = provider.wait()
            result['provider_teardown_ns'] = time.monotonic_ns() - teardown
            sample = json.loads(stem.with_suffix('.json').read_text())
            validate_sample(result, sample, case, stem.with_suffix('.log').read_text())
            result['accepted'] = True
    except Exception as error:
        result['error'] = traceback.format_exc()
    finally:
        for proc in (product, provider):
            if proc is not None and proc.poll() is None:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait()
        for proc in (product, provider):
            if proc is not None:
                for pipe in (proc.stdin, proc.stdout):
                    if pipe is not None:
                        pipe.close()
        result['complete_lifecycle_ns'] = time.monotonic_ns() - began
        stem.with_suffix('.process.json').write_text(json.dumps(result, indent=2))
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--rust', type=Path, required=True)
    p.add_argument('--pi', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--observer-mode', choices=OBSERVER_MODES, default='native-workflow')
    p.add_argument('--pairs', type=int, default=3)
    p.add_argument('--case', choices=CASES, action='append')
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    env = {k: v for k, v in os.environ.items() if not any(s in k.upper() for s in ('TOKEN', 'SECRET', 'CREDENTIAL', 'API_KEY'))}
    env.update(UV_CACHE_DIR='/workspace/.uv-cache')
    manifest = {'observer_mode':args.observer_mode, 'rust': str(args.rust.resolve()), 'rust_sha256': hashlib.file_digest(args.rust.open('rb'), 'sha256').hexdigest(), 'pi': str(args.pi.resolve()), 'pairs': args.pairs, 'cases': args.case or CASES, 'scope': 'product runtime plus driver; sibling provider owned and reaped only by controller; samples include explicitly reported verification phases; evidence files written after cleanup endpoint'}
    (args.output / 'manifest.json').write_text(json.dumps(manifest, indent=2))
    records = []
    for pair in range(args.pairs):
        for case in args.case or CASES:
            for engine in (['rust', 'pi'] if pair % 2 == 0 else ['pi', 'rust']):
                stem = (args.output / f'{pair}-{case}-{engine}').resolve()
                output = stem.with_suffix('.json')
                child_env = env | {'RUSTX_BENCH_OBSERVER_MODE':args.observer_mode, 'RUSTX_BENCH_CASE': case, 'RUSTX_BENCH_OUTPUT': str(output)}
                argv = [str(args.rust.resolve()), TEST, '--exact', '--ignored', '--nocapture'] if engine == 'rust' else ['node', '--conditions=source', 'benchmarks/runtime/pi-http.mjs', str(args.pi.resolve()), case, str(output)]
                record = run(argv, stem, case, child_env, Path('test-support/fake-provider').resolve())
                records.append(record)
                print(json.dumps({k:v for k,v in record.items() if k not in ('provider_report','provider_stdout')}), flush=True)
            comparison = subprocess.run(['node','benchmarks/runtime/compare-http.mjs',
                str(args.output / f'{pair}-{case}-rust.json'), str(args.output / f'{pair}-{case}-pi.json'),
                str(args.output / f'{pair}-{case}-comparison.json')], capture_output=True,text=True)
            if comparison.returncode:
                records.append({'accepted':False,'comparison_error':comparison.stderr})
                (args.output / f'{pair}-{case}-comparison-error.log').write_text(comparison.stderr)
    raise SystemExit(int(any(not r['accepted'] for r in records)))

if __name__ == '__main__':
    main()
