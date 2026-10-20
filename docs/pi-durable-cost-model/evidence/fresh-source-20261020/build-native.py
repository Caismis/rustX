#!/usr/bin/env python3
"""Resource guard and provenance around the unchanged repository builder."""
import datetime
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

root = Path('/workspace/pr485-study')
repo = root / 'repo'
label, sha = sys.argv[1:]
target = root / 'target'
evidence = root / 'evidence'
output = evidence / ('build-' + label)
minimum = 16 * 2**30

def digest(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()

def git(*args):
    return subprocess.check_output(['git', '-C', str(repo), *args], text=True).strip()

def counters():
    stat = os.statvfs(root)
    result = {'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
              'available_bytes': stat.f_bavail * stat.f_frsize}
    for name in ('memory.current', 'memory.peak', 'memory.events'):
        path = Path('/sys/fs/cgroup') / name
        if path.exists():
            result[name] = path.read_text().strip()
    return result

assert not output.exists(), f'output already exists: {output}'
assert not git('status', '--porcelain'), 'isolated checkout must be clean'
assert counters()['available_bytes'] >= minimum, 'insufficient starting reserve'
subprocess.run(['git', '-C', str(repo), 'checkout', '--detach', sha], check=True)
assert git('rev-parse', 'HEAD') == sha
temp = root / 'tmp'
temp.mkdir(exist_ok=True)
env = os.environ.copy()
env.pop('CARGO_ENCODED_RUSTFLAGS', None)
env.update(PATH='/workspace/.cargo/bin:' + env['PATH'],
           CARGO_HOME='/workspace/.cargo', RUSTUP_HOME='/workspace/.rustup',
           CARGO_PROFILE_RELEASE_OPT_LEVEL='3', CARGO_PROFILE_RELEASE_DEBUG='0',
           CARGO_INCREMENTAL='0', CARGO_TERM_VERBOSE='true',
           RUSTFLAGS='-Copt-level=3 -Cdebuginfo=0',
           BINDGEN_EXTRA_CLANG_ARGS='-isystem /usr/lib/gcc/x86_64-linux-gnu/14/include',
           LIBCLANG_PATH='/usr/lib/llvm-19/lib', TMPDIR=str(temp))
argv = [sys.executable, str(repo / 'benchmarks/runtime/build-revision.py'),
        '--repo', str(repo), '--target', str(target), '--output', str(output)]
inputs = ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml',
          'benchmarks/runtime/build-revision.py', 'benchmarks/runtime/controller.py',
          'benchmarks/runtime/compare-revisions.py']
record = {'label': label, 'head': sha, 'tree': git('rev-parse', 'HEAD^{tree}'),
          'source_tree': git('rev-parse', 'HEAD:src'), 'argv': argv,
          'input_sha256': {p: digest(repo / p) for p in inputs},
          'input_git_objects': {p: git('rev-parse', f'HEAD:{p}') for p in
              ['src', 'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml',
               'benchmarks/runtime', 'tests/conformance/agent_loop/runtime_benchmark',
               'tests/conformance/agent_loop/runtime_benchmark.rs',
               'tests/common/provider_emulator.rs', 'test-support/fake-provider']},
          'environment': {k: v for k, v in env.items() if k in
              ('PATH', 'CARGO_HOME', 'RUSTUP_HOME', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS',
               'CARGO_INCREMENTAL', 'CARGO_TERM_VERBOSE', 'BINDGEN_EXTRA_CLANG_ARGS',
               'LIBCLANG_PATH', 'TMPDIR') or k.startswith('CARGO_PROFILE_')},
          'minimum_available_bytes': minimum, 'before': counters()}
status_path = evidence / ('build-' + label + '-orchestration.json')
status_path.write_text(json.dumps(record, indent=2) + '\n')
with (evidence / ('build-' + label + '-driver.stdout')).open('w') as stdout, \
     (evidence / ('build-' + label + '-driver.stderr')).open('w') as stderr, \
     (evidence / ('build-' + label + '-resources.jsonl')).open('w') as resources:
    process = subprocess.Popen(argv, cwd=repo, env=env, stdout=stdout,
                               stderr=stderr, start_new_session=True)
    try:
        while True:
            point = counters()
            resources.write(json.dumps(point) + '\n')
            resources.flush()
            if point['available_bytes'] < minimum and process.poll() is None:
                record['stopped_reason'] = 'available space fell below 16 GiB reserve'
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                break
            if process.poll() is not None:
                break
            time.sleep(5)
    finally:
        record['exit'] = process.wait()
        record['after'] = counters()
        status_path.write_text(json.dumps(record, indent=2) + '\n')
if record['exit']:
    raise SystemExit(record['exit'])
identity = json.loads((output / 'identity.json').read_text())
assert identity['verified'] is True and identity['head'] == sha
assert not git('status', '--porcelain')
artifacts = [json.loads(line) for line in (output / 'build.stdout').read_text().splitlines()
             if line.startswith('{')]
root_artifacts = [row for row in artifacts if row.get('reason') == 'compiler-artifact'
                  and Path(row['target']['src_path']).is_relative_to(repo)]
record['root_profiles'] = []
for row in root_artifacts:
    profile = row['profile']
    assert profile['opt_level'] == '3', profile
    assert profile['debuginfo'] == 0, profile
    item = {'name': row['target']['name'], 'kind': row['target']['kind'],
            'profile': profile, 'fresh': row['fresh'], 'source': row['target']['src_path']}
    if row['target']['kind'] == ['bin'] and row.get('executable'):
        binary = output / row['target']['name']
        shutil.copy2(row['executable'], binary)
        item.update(binary=str(binary), sha256=digest(binary), bytes=binary.stat().st_size)
    record['root_profiles'].append(item)
invocations = [line for line in (output / 'build.stderr').read_text().splitlines()
               if 'Running `' in line and any('--crate-name ' + name in line for name in
                   ('rustx ', 'conformance ', 'bash_supervisor ', 'interactive_supervisor '))]
assert invocations, 'verbose rustc invocation evidence missing'
for line in invocations:
    assert '-C opt-level=3' in line and '-Copt-level=3' in line and '-Cdebuginfo=0' in line, line
(output / 'root-rustc-invocations.txt').write_text('\n'.join(invocations) + '\n')
record.update(verified=True, binary=identity['binary'], sha256=identity['sha256'],
              library_sha256=identity['library_sha256'], after_preservation=counters())
status_path.write_text(json.dumps(record, indent=2) + '\n')
print(json.dumps({k: record[k] for k in ('label', 'head', 'source_tree', 'verified',
      'binary', 'sha256', 'library_sha256', 'after_preservation')}, indent=2))
