#!/usr/bin/env python3
"""Build a preserved release benchmark after invalidating shared root artifacts."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def workspace_libraries(repo, env):
    """Package name -> library target name for every workspace member with a lib target.

    Package names (rustx-types) differ from the target names Cargo puts on
    compiler artifacts (rustx_types): `cargo clean -p` needs the former and
    artifact matching needs the latter.
    """
    metadata = subprocess.check_output(
        ['cargo', 'metadata', '--format-version', '1', '--no-deps'],
        cwd=repo, env=env, text=True)
    libraries = {}
    for package in json.loads(metadata)['packages']:
        for target in package['targets']:
            if target['kind'] == ['lib']:
                libraries[package['name']] = target['name']
                break
    return libraries


def checked_artifacts(records, repo, libraries):
    artifacts = [row for row in records if row.get('reason') == 'compiler-artifact'
                 and (row['target']['kind'] == ['lib']
                      and row.get('target', {}).get('name') in libraries.values()
                      or row['target']['name'] == 'conformance')]
    library = next(row for row in artifacts if row['target']['name'] == 'rustx')
    benchmark = next(row for row in artifacts if row['target']['name'] == 'conformance')
    for row in artifacts:
        if row['fresh'] is not False:
            raise ValueError('workspace libraries and conformance must all be freshly compiled')
        if not str(Path(row['target']['src_path']).resolve()).startswith(str(repo) + '/'):
            raise ValueError('compiler artifact belongs to a different checkout')
    missing = [package for package, target in libraries.items()
               if not any(row['target']['name'] == target for row in artifacts)]
    if missing:
        raise ValueError('no fresh artifacts for workspace libraries: ' + ', '.join(missing))
    return artifacts, library, benchmark


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', type=Path, required=True)
    parser.add_argument('--target', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True,
                        help='new directory; logs, identity and preserved executable')
    args = parser.parse_args()
    repo = args.repo.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = os.environ | {'CARGO_TARGET_DIR': str(args.target.resolve()),
                        'CARGO_BUILD_JOBS': '2'}
    def git(*argv):
        return subprocess.check_output(['git', *argv], cwd=repo, text=True).strip()
    def clean_source():
        if git('status', '--porcelain', '--', 'src', 'crates', 'Cargo.toml', 'Cargo.lock'):
            raise ValueError('production source or Cargo inputs have uncommitted changes')
    clean_source()
    harness = [repo / 'tests/common/provider_emulator.rs',
               repo / 'tests/conformance/agent_loop/runtime_benchmark.rs',
               *sorted(path for path in (repo / 'tests/conformance/agent_loop/runtime_benchmark').rglob('*') if path.is_file())]
    identity = {'head': git('rev-parse', 'HEAD'),
                'production_tree': git('rev-parse', 'HEAD:src'),
                'harness': {str(path.relative_to(repo)): digest(path) for path in harness},
                'start_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                'commands': [],
                'rustc': subprocess.check_output(['rustc', '--version', '--verbose'], env=env, text=True),
                'cargo': subprocess.check_output(['cargo', '--version'], env=env, text=True),
                'build_environment': {key: value for key, value in env.items()
                                      if key in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_JOBS')
                                      or key.startswith('CARGO_PROFILE_RELEASE_')}}
    try:
        libraries = workspace_libraries(repo, env)
        identity['workspace_libraries'] = libraries
        for name, command in (
            ('clean', ['cargo', 'clean', '--release']
                      + ['--package=' + name for name in libraries]),
            ('build', ['cargo', 'test', '--release', '--locked', '--test',
                       'conformance', '--no-run', '--message-format=json']),
        ):
            identity['commands'].append(command)
            with (output / (name + '.stdout')).open('w') as stdout, \
                 (output / (name + '.stderr')).open('w') as stderr:
                result = subprocess.run(command, cwd=repo, env=env,
                                        stdout=stdout, stderr=stderr, check=False)
            identity[name + '_exit'] = result.returncode
            result.check_returncode()
        records = [json.loads(line) for line in (output / 'build.stdout').read_text().splitlines()
                   if line.startswith('{')]
        artifacts, library, benchmark = checked_artifacts(records, repo, libraries)
        identity['root_artifacts'] = [
            {'name': row['target']['name'], 'kind': row['target']['kind'],
             'source_path': row['target']['src_path'], 'fresh': row['fresh']}
            for row in artifacts]
        clean_source()
        if git('rev-parse', 'HEAD') != identity['head']:
            raise ValueError('checkout changed during build')
        if any(digest(repo / path) != expected for path, expected in identity['harness'].items()):
            raise ValueError('harness changed during build')
        binary = output / 'conformance'
        shutil.copy2(benchmark['executable'], binary)
        identity['binary'] = str(binary)
        identity['sha256'] = digest(binary)
        identity['library_sha256'] = digest(Path(next(
            path for path in library['filenames'] if path.endswith('.rlib'))))
        identity['verified'] = True
    finally:
        identity['end_utc'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        (output / 'identity.json').write_text(json.dumps(identity, indent=2) + '\n')
    print(json.dumps(identity, indent=2))


if __name__ == '__main__':
    main()
