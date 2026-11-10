#!/usr/bin/env python3
"""Run the declared cohorts serially, retain failures, and audit all final raw data."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import traceback

CACHE = Path('/home/capi/.cache/rustx-pr485-next')
REPO = Path('/home/capi/.codex/worktrees/pr485-pi-performance/rustX')
EXPECTED_HEAD = 'a39f343b54a46c8e5527dad88842cb8a6d46691b'
EXPECTED_TREE = '7f4952a1e2473ce1a97e2622c742789775a685f7'
EXPECTED_SHA = '19c52cb8b539abfc0a259d6493ad08b65df00a1e4cc782284cdf562ffa866cf7'
BINARY_B = Path('/home/capi/.cache/rustx-pr485-evidence/candidate-final-build/conformance')
SHA_B = 'f80f3b7ba0f2be6035c7459c87cf04e69ec596e64ccba0d72d43ddce7d8ebdb3'
BINARY_D = CACHE / 'candidate-d-build/conformance'
SHA_D = '8ec26a4a08bde085f46ee40159d3870f7ead675bec39f42cb4f3e55df1d1d233'
PI = Path('/home/capi/Documents/codes/pi-rustx-benchmark')


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def sha(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def build_identity(binary, expected_sha, *, candidate=False):
    path = binary.parent / 'identity.json'
    identity = json.loads(path.read_text())
    require(identity.get('verified') is True and identity.get('build_exit') == 0, f'unverified build: {path}')
    require(identity.get('binary') == str(binary), f'build binary path mismatch: {path}')
    require(identity.get('sha256') == expected_sha == sha(binary), f'actual/build/expected binary SHA mismatch: {binary}')
    require(isinstance(identity.get('head'), str) and isinstance(identity.get('production_tree'), str), f'missing recorded source identity: {path}')
    recorded_tree = subprocess.check_output(['git', 'rev-parse', identity['head'] + ':src'], cwd=REPO, text=True).strip()
    require(recorded_tree == identity['production_tree'], f'build source tree mismatch: {path}')
    if candidate:
        require(identity['head'] == EXPECTED_HEAD and recorded_tree == EXPECTED_TREE, 'candidate is not the declared a39f343 source/tree')
        require(isinstance(identity.get('harness'), dict) and identity['harness'], 'candidate build does not record benchmark sources')
        for relative, expected in identity['harness'].items():
            require(sha(REPO / relative) == expected, f'current benchmark source differs from measured build: {relative}')
    return {'path': str(path), 'sha256': sha(path), 'binary': str(binary), 'actual_binary_sha256': expected_sha, 'recorded_source_tree_verified': True, 'data': identity}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rust', required=True, type=Path)
    parser.add_argument('--stage', choices=['e-direct', 'final'], required=True)
    args = parser.parse_args()
    rust = args.rust.resolve()
    base = CACHE / ('candidate-e-direct' if args.stage == 'e-direct' else 'final-cohorts')
    base.mkdir(exist_ok=False)
    steps = [{'name': 'identity-preflight', 'kind': 'preflight', 'status': 'pending'}]
    def add(name, argv, *, audit=False, stdout=None):
        steps.append({'name': name, 'kind': 'audit' if audit else 'cohort', 'argv': argv, 'cwd': str(REPO), 'status': 'pending', 'stdout': str(stdout or base / (name + '.stdout')), 'stderr': str(base / (name + '.stderr'))})
    python = sys.executable
    if args.stage == 'e-direct':
        for mode, label in [('no-live-subscriber', 'no-live'), ('live-subscriber', 'live')]:
            name = 'direct-' + label
            add(name, [python, 'benchmarks/runtime/compare-revisions.py', '--before', str(BINARY_D), '--after', str(rust), '--output', str(base / name), '--pairs', '3', '--observer-mode', mode])
        add('recompute-direct', [python, 'docs/pi-durable-cost-model/evidence/read-cache-trial-20261027/recompute-direct.py', '--root', str(base)], audit=True, stdout=base / 'recomputed.json')
    else:
        for mode, label in [('no-live-subscriber', 'no-live'), ('live-subscriber', 'live')]:
            name = 'pi-' + label
            add(name, [python, 'benchmarks/runtime/controller.py', '--rust', str(rust), '--pi', str(PI), '--output', str(base / name), '--pairs', '3', '--observer-mode', mode])
        for cold in (False, True):
            name = 'append-cold' if cold else 'append-warm'
            add(name, [python, 'benchmarks/runtime/compare-append.py', '--before', str(BINARY_B), '--after', str(rust), '--output', str(base / name), '--pairs', '3', *(['--cold'] if cold else [])])
        add('three-engine', [python, 'benchmarks/runtime/deepseek-compare.py', '--rust', str(rust), '--pi', str(PI), '--deepseek', str(CACHE / 'deepseek-checkout'), '--output', str(CACHE / 'three-engine-final'), '--repetitions', '3'])
        add('audit-pi', [python, str(CACHE / 'recompute-pi.py'), '--root', str(base), '--identity', str(rust.parent / 'identity.json'), '--expect-rust-sha256', EXPECTED_SHA], audit=True, stdout=base / 'pi-recomputed.json')
        add('audit-append', [python, str(CACHE / 'audit-final-append.py'), '--root', str(base), '--output', str(base / 'append-audit.json'), '--binary-verification', 'strict', '--before-identity', str(BINARY_B.parent / 'identity.json'), '--after-identity', str(rust.parent / 'identity.json'), '--expect-before-sha256', SHA_B, '--expect-after-sha256', EXPECTED_SHA], audit=True)
        add('audit-three-engine', [python, str(CACHE / 'recompute-three-engine.py'), '--dataset', str(CACHE / 'three-engine-final'), '--output', str(base / 'three-engine-audit.json'), '--expected-rust-sha256', EXPECTED_SHA, '--expected-harness-revision', EXPECTED_HEAD], audit=True)
    plan = {'stage': args.stage, 'created_utc': now(), 'runner': str(Path(__file__).resolve()), 'runner_sha256': sha(__file__), 'repo_cwd': str(REPO), 'expected_candidate': {'head': EXPECTED_HEAD, 'production_tree': EXPECTED_TREE, 'sha256': EXPECTED_SHA, 'binary': str(rust)}, 'explicit_environment': {'UV_OFFLINE': '1'}, 'evidence_roots': [str(base)] + ([str(CACHE / 'three-engine-final')] if args.stage == 'final' else []), 'steps': steps, 'status': 'pending'}
    def save():
        (base / 'plan.json').write_text(json.dumps(plan, indent=2) + '\n')
        (base / 'commands.json').write_text(json.dumps(steps, indent=2) + '\n')
    save()
    for index, step in enumerate(steps):
        step.update(status='running', start_utc=now()); plan['status'] = 'running'; save()
        try:
            if step['kind'] == 'preflight':
                candidate = build_identity(rust, EXPECTED_SHA, candidate=True)
                baseline = build_identity(BINARY_D if args.stage == 'e-direct' else BINARY_B, SHA_D if args.stage == 'e-direct' else SHA_B)
                require(candidate['data']['harness'] == baseline['data']['harness'], 'baseline/candidate benchmark sources differ')
                require(candidate['data']['rustc'] == baseline['data']['rustc'], 'baseline/candidate compiler identities differ')
                plan['build_identities'] = {'candidate': candidate, 'baseline': baseline}
                code = 0
            else:
                with Path(step['stdout']).open('w') as out, Path(step['stderr']).open('w') as err:
                    result = subprocess.run(step['argv'], cwd=REPO, stdout=out, stderr=err, env=os.environ | {'UV_OFFLINE': '1'})
                code = result.returncode
            step.update(exit=code, status='complete' if code == 0 else 'failed', end_utc=now())
        except (Exception, KeyboardInterrupt) as error:
            code = 130 if isinstance(error, KeyboardInterrupt) else 1
            step.update(exit=code, status='failed', end_utc=now(), error=f'{type(error).__name__}: {error}')
            (base / (step['name'] + '.exception.log')).write_text(traceback.format_exc())
        if code:
            for later in steps[index + 1:]:
                later.update(status='not_run', reason='preceding required step failed: ' + step['name'])
            plan.update(status='failed', end_utc=now()); save()
            print(json.dumps(step), flush=True)
            return code if code > 0 else 1
        save(); print(json.dumps(step), flush=True)
    plan.update(status='complete', end_utc=now()); save()
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
