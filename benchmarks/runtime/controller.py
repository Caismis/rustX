#!/usr/bin/env python3
"""Own product and local HTTP/SSE provider as siblings; retain every failed sample."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import signal
import traceback
import time
import urllib.request

TEST = 'agent_loop::runtime_benchmark::measure_runtime'
CASES = ['short-small', 'short-large', 'stream', 'large', 'tool', 'cancel']


def control(base, path):
    with urllib.request.urlopen(urllib.request.Request(base + path, data=b'', method='POST'), timeout=30) as r:
        return json.load(r)


def run(argv, stem, case, env, project):
    began = time.monotonic_ns()
    product = provider = None
    result = {'argv': argv, 'case': case, 'accepted': False}
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
            assert result['product_exit'] == result['provider_exit'] == 0
            assert result['provider_report']['ok'] is True
            assert sample['oracle'] == 'passed'
            expected_requests = {'short-small':40,'short-large':96,'tool':2}.get(case,1)
            assert result['provider_report']['requestCount'] == expected_requests
            wire = sample.get('wire_requests', sample.get('requests'))
            assert isinstance(wire,list) and len(wire) == expected_requests
            assert wire == result['provider_report']['requests']
            expected_history={'short-small':8,'short-large':64}.get(case,0)
            expected_measured=32 if expected_history else 1
            assert sample['history']==expected_history and sample['measured']==expected_measured
            assert len(sample['samples'])==expected_history+expected_measured
            for i,point in enumerate(sample['samples']):
                assert point['turn']==i and point['measured']==(i>=expected_history)
                assert isinstance(point['settled_us'],int) and point['settled_us']>=0
                assert isinstance(point['accepted_us'],int) and point['accepted_us']>=0
                assert set(point['process'])=={'stat','io','smaps_rollup'}
            elapsed=[p['elapsed_us'] for p in sample['lifecycle']]
            assert all(isinstance(v,int) and v>=0 for v in elapsed) and elapsed==sorted(elapsed)
            if 'measurement_version' in sample:
                assert sample['measurement_version'] == 3
                assert sample['verified_live_requests'] == sample['verified_reopened_requests'] == expected_requests
                assert len(sample['live_requests']) == len(sample['reopened_requests']) == expected_requests
                assert sample['live_requests'] == sample['reopened_requests']
                assert f'test {TEST} ... ok' in stem.with_suffix('.log').read_text()
            else:
                assert len(sample['captures']) == len(sample['reopenedRequests']) == expected_requests
                assert [c['messages'] for c in sample['captures']] == sample['reopenedRequests']
            assert sample['lifecycle'][-1]['phase'] == 'after_cleanup'
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
    p.add_argument('--pairs', type=int, default=3)
    p.add_argument('--case', choices=CASES, action='append')
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    env = {k: v for k, v in os.environ.items() if not any(s in k.upper() for s in ('TOKEN', 'SECRET', 'CREDENTIAL', 'API_KEY'))}
    env.update(UV_CACHE_DIR='/workspace/.uv-cache')
    manifest = {'rust': str(args.rust.resolve()), 'rust_sha256': hashlib.file_digest(args.rust.open('rb'), 'sha256').hexdigest(), 'pi': str(args.pi.resolve()), 'pairs': args.pairs, 'cases': args.case or CASES, 'scope': 'product runtime plus driver; sibling provider owned and reaped only by controller; samples include explicitly reported verification phases; evidence files written after cleanup endpoint'}
    (args.output / 'manifest.json').write_text(json.dumps(manifest, indent=2))
    records = []
    for pair in range(args.pairs):
        for case in args.case or CASES:
            for engine in (['rust', 'pi'] if pair % 2 == 0 else ['pi', 'rust']):
                stem = (args.output / f'{pair}-{case}-{engine}').resolve()
                output = stem.with_suffix('.json')
                child_env = env | {'RUSTX_BENCH_CASE': case, 'RUSTX_BENCH_OUTPUT': str(output)}
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
