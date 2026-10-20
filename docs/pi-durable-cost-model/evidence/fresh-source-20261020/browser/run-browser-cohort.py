#!/usr/bin/env python3
"""Unchanged v3 fixture; independent preflight, identity and cohort evidence."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

root = Path('/workspace/pr485-study')
repo = root / 'repo'
fixture = root / 'inspector-fixture'
evidence = root / 'evidence'
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--candidate', choices=('geometry', 'scoped'), required=True)
p.add_argument('--output', type=Path, required=True)
a = p.parse_args()
a.output = a.output.resolve()
a.output.mkdir(parents=True, exist_ok=False)
expected = json.loads((evidence / 'handoff-fixture-identity.json').read_text())
frontend = json.loads((evidence / 'common-frontend.json').read_text())
image = 'mcr.microsoft.com/playwright:v1.63.0-noble@sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7'

def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()

def sha(path):
    with Path(path).open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()

def fixture_hashes():
    return {x.name: sha(x) for x in fixture.iterdir() if x.is_file()}

def space(check=True):
    docker_root = subprocess.check_output(['docker', 'info', '--format', '{{.DockerRootDir}}'], text=True).strip()
    result = []
    for path in (str(root), docker_root):
        s = os.statvfs(path)
        result.append({'path': path, 'device': os.stat(path).st_dev,
            'usable_bytes': s.f_bavail * s.f_frsize, 'total_bytes': s.f_blocks * s.f_frsize})
    if check:
        assert all(x['usable_bytes'] > 5368709120 for x in result), result
    return {'paths': result, 'distinct_backing_filesystems': len({x['device'] for x in result})}

def assert_idle():
    observed = []
    forbidden = {'cargo', 'rustc', 'rust-lld', 'cc1', 'cc1plus', 'xz', 'gzip', 'perf', 'strace'}
    for entry in Path('/proc').iterdir():
        if not entry.name.isdigit() or int(entry.name) == os.getpid():
            continue
        try:
            comm = (entry / 'comm').read_text().strip()
            if comm in forbidden:
                observed.append({'pid': int(entry.name), 'comm': comm})
            args = (entry / 'cmdline').read_bytes().replace(b'\0', b' ').decode(errors='replace')
            owned_node = comm == 'node' and (str(fixture / 'run.mjs') in args
                or str(repo) in args and any(s in args for s in ('vite', 'host/main.ts', 'workspace-host')))
            if comm in ('conformance', 'inspector-host', 'fake-provider') or owned_node:
                observed.append({'pid': int(entry.name), 'comm': comm, 'args': args})
        except (OSError, ProcessLookupError):
            pass
    assert not observed, observed
    containers = [name for name in subprocess.check_output(['docker', 'ps', '--format', '{{.Names}}'], text=True).splitlines()
                  if name.startswith('pr485-inspector-')]
    assert not containers, containers
    return {'utc': now(), 'forbidden_or_timed_processes': observed, 'active_benchmark_containers': containers}

def restore(label):
    build = evidence / ('build-' + label)
    identity = json.loads((build / 'identity.json').read_text())
    host = json.loads((build / 'host/identity.json').read_text())
    preserved = json.loads((build / 'preserved-artifacts.json').read_text())
    assert identity['verified'] is True and host['exit_code'] == 0
    assert host['extern']['rustx']['sha256'] == identity['library_sha256']
    assert sha(build / 'host/inspector-host') == host['binary_sha256']
    libraries = [item for item in preserved['artifacts'] if item['kind'] == ['lib']
                 and item['name'] == 'rustx' and item['preserved'].endswith('.rlib')]
    assert len(libraries) == 1 and sha(libraries[0]['preserved']) == identity['library_sha256']
    copied = []
    for item in preserved['artifacts']:
        if item['kind'] not in (['bin'], ['test'], ['shared-library']):
            continue
        src, dest = Path(item['preserved']), Path(item['target'])
        assert sha(src) == item['sha256']
        if not dest.exists() or sha(dest) != item['sha256']:
            dest.parent.mkdir(parents=True, exist_ok=True)
            staging = dest.with_name(dest.name + '.restore')
            shutil.copy2(src, staging)
            staging.replace(dest)
            copied.append(str(dest))
        assert sha(dest) == item['sha256']
    return identity, host, copied

manifest = {'start_utc': now(), 'comparison': ['baseline', a.candidate],
    'pairs': 3, 'cases': ['short-small', 'short-large', 'details-resync'],
    'fixture_commit': 'bddf435705f545f9b05ccd9b637d0fe85d4baa47', 'fixture_sha256': expected,
    'frontend': frontend, 'browser_image': image, 'runs': [],
    'interpretation': 'Conditional Rust/Rust comparison. Fixed raw performance_eligible:false is preserved; successful producer oracle is not formal independent acceptance.',
    'wire_drain_scope': 'Observed wire coverage at exact cut, current epoch no pending RPC, two requestAnimationFrame callbacks with unchanged generation. Reply observation precedes route.send. Not proof of all React queues, offscreen rendering or completed paints.'}
manifest_path = a.output / 'cohort-manifest.json'

def save():
    manifest_path.write_text(json.dumps(manifest, indent=2) + '\n')

save()
env = os.environ | {'TMPDIR': str(root / 'tmp'), 'TZ': 'UTC'}
assert not env.get('LD_PRELOAD')
assert subprocess.check_output(['node', '--version'], text=True).strip() == 'v24.19.0'
assert os.sysconf('SC_CLK_TCK') == 100
manifest['runtime_environment'] = {'node': '24.19.0', 'TZ': env['TZ'], 'TMPDIR': env['TMPDIR'],
    'LD_PRELOAD': env.get('LD_PRELOAD'), 'clock_ticks': os.sysconf('SC_CLK_TCK'),
    'cpu_max': Path('/sys/fs/cgroup/cpu.max').read_text().strip(),
    'memory_max': Path('/sys/fs/cgroup/memory.max').read_text().strip()}
save()
for case in manifest['cases']:
    for pair in range(3):
        order = ['baseline', a.candidate]
        if (pair + (case != 'short-small')) % 2:
            order.reverse()
        for label in order:
            run = {'label': label, 'case': case, 'pair': pair, 'preflight_started_utc': now()}
            manifest['runs'].append(run)
            save()
            try:
                assert fixture_hashes() == expected
                for relative, digest in frontend['dist_sha256'].items():
                    assert sha(repo / 'web-console/dist' / relative) == digest
                for relative, digest in frontend['native_node_modules_sha256'].items():
                    assert sha(repo / 'web-console' / relative) == digest
                run['idle_evidence'] = assert_idle()
                run['space_before'] = space()
                run['image_inspect'] = json.loads(subprocess.check_output(['docker', 'image', 'inspect', image], text=True))
                identity, host, copied = restore(label)
                run.update(native_build=identity, host_build=host, helpers_restored=copied)
                (repo / 'source-identity.json').write_text(json.dumps({'head': identity['head']}) + '\n')
                run['space_at_product_start'] = space()
                out = a.output / f'{pair}-{case}-{label}'
                argv = ['node', '--experimental-transform-types', str(fixture / 'run.mjs'),
                        str(repo), str(evidence / ('build-' + label) / 'host/inspector-host'), case, str(out)]
                run.update(argv=argv, product_start_utc=now(), product_start_monotonic_ns=time.monotonic_ns())
                save()
                with (a.output / (out.name + '.stdout')).open('w') as stdout, (a.output / (out.name + '.stderr')).open('w') as stderr:
                    result = subprocess.run(argv, cwd=repo, env=env, stdout=stdout, stderr=stderr)
                run.update(exit=result.returncode, product_end_utc=now(), product_end_monotonic_ns=time.monotonic_ns())
                run['space_after'] = space(check=False)
                assert fixture_hashes() == expected
                assert result.returncode == 0, 'fixture failed; stop this cohort and inspect retained failure'
                sample = json.loads((out / 'sample.json').read_text())
                assert sample['oracle'] == 'passed' and sample['performance_eligible'] is False
                assert sample['source'] == identity['head'] and sample['native_binary_sha256'] == host['binary_sha256']
                assert sample['fixture_source'] == expected
                run['sample_sha256'] = sha(out / 'sample.json')
                run['accepted_by_producer'] = True
                save()
            except BaseException as error:
                run.update(error=str(error), stopped_utc=now())
                manifest['stopped_on_failure'] = True
                save()
                raise
manifest['end_utc'] = now()
manifest['completed_products'] = len(manifest['runs'])
save()
print(json.dumps({'output': str(a.output), 'products': len(manifest['runs'])}))
