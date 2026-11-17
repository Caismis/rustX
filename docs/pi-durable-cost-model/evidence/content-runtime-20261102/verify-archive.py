#!/usr/bin/env python3
"""Safely extract all incremental-attempt evidence and replay retained arithmetic only.

No product/build/profiler runs. Historical executable/source paths are provenance.
Use a fresh work directory; the receipt is written only by an actual invocation.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import subprocess
import sys
import tarfile

sys.dont_write_bytecode = True
FORMAT = 'rustx-content-runtime-raw-evidence-v1'
DEFAULT_EVIDENCE = Path('/home/capi/.codex/worktrees/pr485-pi-performance/rustX/docs/pi-durable-cost-model/evidence/content-runtime-20261102')
DEFAULT_WORK = Path('/home/capi/.cache/rustx-pr485-content-20261102-portable-verification')
COMMANDS = {
    'replication': ['python3', 'replication-audit.py', '--root', '.', '--raw-only', '--output', '../replication-raw-audit.json'],
    'formal': ['python3', 'direct-audit.py', '--root', '.', '--raw-only', '--output', '../direct-raw-audit.json'],
    'diagnostic': ['python3', 'diagnostic-audit-corrected.py', '--root', '.', '--raw-only', '--formal-audit', '../direct-raw-audit.json', '--output', '../diagnostic-raw-audit.json'],
    'summary': ['python3', 'summarize-results.py', '--root', '.', '--audit', '../direct-raw-audit.json', '--output', '../report-data.json', '--markdown', '../report-tables.md'],
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def now():
    return datetime.now(timezone.utc).isoformat()


def unique_object(pairs):
    value = {}
    for key, item in pairs:
        require(key not in value, 'duplicate JSON key: ' + key)
        value[key] = item
    return value


def load(path):
    return json.loads(Path(path).read_text(), object_pairs_hook=unique_object,
                      parse_constant=lambda value: (_ for _ in ()).throw(ValueError('nonfinite JSON: ' + value)))


def sha(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def integer(value, label):
    require(type(value) is int and value >= 0, 'invalid nonnegative integer: ' + label)


def safe_name(name):
    require(isinstance(name, str) and name and '\\' not in name and '\x00' not in name,
            'unsafe archive path: ' + repr(name))
    path = PurePosixPath(name)
    require(not path.is_absolute() and path.as_posix() == name and
            all(part not in ('', '.', '..') for part in name.split('/')),
            'unsafe archive path: ' + name)


def fingerprint(path):
    item = Path(path).lstat()
    require(stat.S_ISREG(item.st_mode), 'not a regular file: ' + str(path))
    return item.st_dev, item.st_ino, item.st_size, item.st_mtime_ns, item.st_ctime_ns


def verify_tree(root, expected):
    seen, directories = {}, set()
    allowed_directories = {str(parent) for name in expected for parent in PurePosixPath(name).parents if str(parent) != '.'}
    for directory, folders, files in os.walk(root, followlinks=False):
        for name in folders:
            path = Path(directory) / name
            require(stat.S_ISDIR(path.lstat().st_mode), 'unexpected non-directory/symlink: ' + str(path))
            directories.add(path.relative_to(root).as_posix())
        for name in files:
            path = Path(directory) / name
            state = fingerprint(path)
            relative = path.relative_to(root).as_posix()
            require(relative in expected, 'unexpected extracted file: ' + relative)
            seen[relative] = {'bytes': state[2], 'sha256': sha(path)}
            require(fingerprint(path) == state, 'extracted file changed while hashing: ' + relative)
    require(directories == allowed_directories and seen == expected, 'complete extracted inventory/hash/size differs')


def extract_and_verify(archive_path, summary, root):
    root.mkdir(mode=0o700)  # Refuse reuse of a pre-existing extraction tree.
    observed, total = {}, 0
    maximum = summary['raw_bytes'] + summary['manifest_bytes']
    with tarfile.open(archive_path, 'r|xz') as archive:
        for member in archive:
            safe_name(member.name)
            require(member.type in (tarfile.REGTYPE, tarfile.AREGTYPE) and not member.issparse(),
                    'unexpected nonregular archive member: ' + member.name)
            require(member.name not in observed, 'duplicate archive member: ' + member.name)
            integer(member.size, 'member size')
            total += member.size
            require(total <= maximum and len(observed) < summary['raw_files'] + 1,
                    'archive exceeds declared payload size/count')
            destination = root / member.name
            destination.parent.mkdir(parents=True, exist_ok=True)
            source = archive.extractfile(member)
            require(source is not None, 'missing archive bytes: ' + member.name)
            digest, length = hashlib.sha256(), 0
            with destination.open('xb') as stream:
                while block := source.read(1024 * 1024):
                    stream.write(block)
                    digest.update(block)
                    length += len(block)
            require(length == member.size, 'truncated member: ' + member.name)
            observed[member.name] = {'bytes': length, 'sha256': digest.hexdigest()}
    manifest_record = {'bytes': summary['manifest_bytes'], 'sha256': summary['manifest_sha256']}
    require(observed.get('MANIFEST.json') == manifest_record, 'manifest differs from external summary')
    manifest = load(root / 'MANIFEST.json')
    require(manifest['format'] == summary['format'] == FORMAT, 'archive format differs')
    for field in ('phases', 'reference_pins', 'replay_commands', 'strict_report_sha256'):
        require(manifest[field] == summary[field], 'summary/manifest field differs: ' + field)
    require(manifest['replay_commands'] == COMMANDS, 'unexpected portable command plan')
    entries = manifest['files']
    require(isinstance(entries, dict) and len(entries) == summary['raw_files'], 'manifest file count differs')
    expected = {}
    for name, item in entries.items():
        safe_name(name)
        require(name != 'MANIFEST.json', 'manifest recursively includes itself')
        integer(item['bytes'], 'manifest member bytes')
        require(isinstance(item['sha256'], str) and re.fullmatch('[0-9a-f]{64}', item['sha256']), 'invalid member hash')
        expected[name] = {key: item[key] for key in ('bytes', 'sha256')}
    require(sum(item['bytes'] for item in expected.values()) == summary['raw_bytes'], 'payload size differs')
    expected['MANIFEST.json'] = manifest_record
    require(observed == expected, 'archive has missing/extra/changed members')
    return manifest, expected


def verify_wal_parser_correction(root):
    receipt = load(root / 'wal-parser-correction.json')
    pre = load(root / 'premeasurement.json')
    require(receipt['all_original_frozen_scripts_unchanged'] is True and receipt['frozen_scripts'] == pre['scripts']
            and len(pre['scripts']) == 13 and receipt['premeasurement_sha256'] == sha(root / 'premeasurement.json')
            and all(sha(root / name) == digest for name,digest in pre['scripts'].items()), 'original frozen scripts changed')
    failed = load(root / 'diagnostic-audit.json')
    require(failed['accepted'] is False and failed['errors'] == ['wal-baseline-short-small: ValueError: WAL frame headers and page payload counts differ']
            and receipt['failed_diagnostic_audit_sha256'] == sha(root / 'diagnostic-audit.json'), 'original WAL audit differs')
    old = r'(\d+)\) = (\d+)$'
    new = r'(\d+)\s*\)\s*=\s*(\d+)$'
    import difflib
    require(set(receipt['patches']) == {'analyze-wal-verified.py','diagnostic-audit.py'}, 'WAL correction inventory differs')
    for original,corrected in [('analyze-wal-verified.py','analyze-wal-corrected.py'),('diagnostic-audit.py','diagnostic-audit-corrected.py')]:
        source = (root / original).read_text()
        expected = source.replace(old,new).replace("f'wal-summary-{label}.json'", "f'wal-summary-{label}-corrected.json'")
        item = receipt['patches'][original]
        require(source.count(old) == 1 and (root / corrected).read_text() == expected
                and item['corrected'] == corrected and item['original_sha256'] == sha(root / original)
                and item['corrected_sha256'] == sha(root / corrected), 'WAL correction exceeds parser whitespace and new summary names')
        require(item['diff'] == ''.join(difflib.unified_diff(source.splitlines(True),expected.splitlines(True),fromfile=original,tofile=corrected)), 'WAL correction diff differs')


def strict_reports(root):
    verify_wal_parser_correction(root)
    formal, diagnostic, report = (load(root / name) for name in ('direct-audit.json', 'diagnostic-audit-final.json', 'report-data.json'))
    for value, script in ((formal, 'direct-audit.py'), (diagnostic, 'diagnostic-audit-corrected.py')):
        require(value['accepted'] is True and value['errors'] == [] and value['mode'] == 'strict'
                and value['strict_identity_verified'] is True, 'preserved strict report not accepted')
        require(value['script_sha256'] == sha(root / script), 'strict auditor changed')
    require(formal['raw_only'] is False and set(formal['datasets']) == {'direct'}, 'unexpected formal scope')
    direct = formal['datasets']['direct']
    require((direct['derived_products'], direct['valid_comparisons'], direct['checked_requests']) == (144, 72, 1692), 'formal coverage differs')
    require(formal['validation']['successful_complete_queues'], 'bound correctness queue absent')
    candidate = load(root / 'candidate-complete-build/identity.json')
    binding = candidate['validation']
    require(binding['directory'] == formal['validation']['bound_directory']
            and binding['directory'] in formal['validation']['successful_complete_queues'], 'candidate/strict correctness binding differs')
    safe_name(binding['directory'])
    require('/' not in binding['directory'], 'validation directory is not an immediate child')
    for name, digest in binding['files'].items():
        safe_name(name)
        require(sha(root / binding['directory'] / name) == digest, 'bound correctness file changed')
    require(diagnostic['recorded_build_identity_verified'] is True and diagnostic['actual_binary_verified'] is True
            and diagnostic['product_count'] == 8 and not diagnostic.get('profiles') and all(len(diagnostic[key]) == 4 for key in ('columns', 'wal')), 'strict diagnostic scope/identity differs')
    require(diagnostic['formal_audit']['mode'] == 'strict' and diagnostic['formal_audit']['sha256'] == sha(root / 'direct-audit.json'), 'strict diagnostic/formal binding differs')
    require(report['accepted'] is True and report['script_sha256'] == sha(root / 'summarize-results.py')
            and report['current_audit_sha256'] == sha(root / 'direct-audit.json'), 'strict summary binding differs')
    require((report['current_rust_products'], report['fresh_paired_products'], report['fixed_pi_products'], report['fixed_deepseek_products']) == (72, 144, 60, 12), 'summary product count differs')
    require(report['reference_pin_sha256'] == sha(root / 'reference-pins/fixed-references.json')
            and report['references'] == load(root / 'reference-pins/fixed-references.json'), 'summary reference pins differ')
    return formal, diagnostic, report


def verify_scope(root, summary):
    phase = summary['phases']
    require((phase['formal_products'], phase['diagnostic_products'], phase['new_pi_products'], phase['new_deepseek_products']) == (144, 8, 0, 0), 'round scope differs')
    for label, folder in (('baseline', 'baseline-build'), ('candidate', 'candidate-complete-build')):
        identity = load(root / folder / 'identity.json')
        require(phase[label] == {key: identity[key] for key in ('head', 'production_tree', 'sha256')}, 'build identity differs')
        require(not (root / folder / 'conformance').exists(), 'compiled product was archived')
    require((phase['baseline_schema'], phase['candidate_schema']) == (87, 88), 'schema scope differs')
    require((phase['paired_comparisons'], phase['checked_request_pairs'], phase['cpu_affinity'],
             phase['profile_products'], phase['supplementary_products']) == (72, 1692, [2, 8], 0, 24), 'pair/affinity/diagnostic scope differs')
    protocol = load(root / 'direct-cohorts/commands.json')['protocol']
    require(protocol['cpu_affinity'] == [2, 8] and protocol['pairs_per_cell'] == 6
            and protocol['products'] == 144 and protocol['paired_comparisons'] == 72
            and protocol['checked_request_pairs'] == 1692
            and protocol['order'] == ['before_after', 'after_before'] * 3, 'formal protocol differs')
    pins = load(root / 'reference-pins/fixed-references.json')
    require(pins == summary['reference_pins'], 'reference pins differ')
    safe_name(pins['reference_audit'])
    require('/' not in pins['reference_audit'] and sha(root / 'reference-pins' / pins['reference_audit']) == pins['reference_audit_sha256'], 'reference audit hash differs')
    previous, receipt = (load(root / 'reference-pins' / name) for name in ('previous-summary.json', 'previous-portable-verification.json'))
    require(previous['archive_sha256'] == receipt['archive_sha256'] == pins['reference_archive_sha256']
            and previous['archive_bytes'] == receipt['archive_bytes'] == pins['reference_archive_bytes']
            and previous['manifest_sha256'] == receipt['manifest_sha256'] == pins['archive_manifest_sha256'], 'prior archive pin differs')
    require(receipt['accepted'] is True and receipt['errors'] == []
            and receipt['strict_source_report_sha256'] == pins['reference_audit_sha256']
            and receipt['summary_sha256'] == sha(root / 'reference-pins/previous-summary.json'), 'prior portable receipt differs')
    require(phase['baseline']['head'] == 'a664c5a96546c3e49c5638c3c959b08df3c77fec' and phase['baseline']['sha256'] == '62c2e96a5a611de0df64e35065ae9bfd04a794bafef882a2214244ff1cb4587e', 'fixed reference/baseline differs')
    before = load(root / 'baseline-build/identity.json')
    original = load(root / 'baseline-build/original-identity.json')
    require({key: value for key, value in before.items() if key != 'binary'}
            == {key: value for key, value in original.items() if key != 'binary'}, 'copied baseline differs from original receipt')
    require(before['production_tree'] == 'dd0b2c579159d532420979209157e94d32d80131', 'baseline source tree differs')
    candidate = load(root / 'candidate-complete-build/identity.json')
    fixtures = load(root / 'direct-cohorts/commands.json')['source_before']['fixtures']
    require(all(name not in fixtures or fixtures[name] == digest for name, digest in candidate['harness'].items()), 'harness/fixture hashes conflict')
    for relative, digest in {**fixtures, **candidate['harness']}.items():
        safe_name(relative)
        require(sha(root / 'frozen-harness' / relative) == digest, 'frozen harness differs: ' + relative)
    return {'formal_products': 144, 'diagnostic_products': 8, 'new_pi_products': 0, 'new_deepseek_products': 0,
            'paired_comparisons': 72, 'checked_request_pairs': 1692, 'cpu_affinity': [2, 8],
            'profile_products': 0, 'supplementary_products': 24,
            'fixed_reference_archive_sha256': pins['reference_archive_sha256'],
            'previous_raw_archive_recomputed_here': False,
            'scope': 'Prior fixed report, archive identity and prior portable receipt retained; old raw is a separate content-hash-pinned dependency, not re-derived by this round.'}


def copy(value):
    return json.loads(json.dumps(value, allow_nan=False))


def compare_formal(raw, strict):
    require(raw['accepted'] is True and raw['errors'] == [] and raw['mode'] == 'raw-only'
            and raw['raw_only'] is True and raw['strict_identity_verified'] is False, 'formal replay identity mode differs')
    left, right = copy(raw), copy(strict)
    for value in (left, right):
        for field in ('mode', 'raw_only', 'strict_identity_verified', 'created_utc'):
            value.pop(field)
        # Current committed file bytes are checked only by the preserved strict run.
        # Retained source/check maps, failures and bound successful queues must agree.
        for run in value['validation']['runs']:
            run.pop('source_binding')
    require(left == right, 'formal complete numerical/admission/retained-identity records differ')
    return {'products': 144, 'comparisons': 72, 'checked_requests': 1692,
            'comparison': 'Entire report except mode/current-host source assertion/timestamp; all dataset fields, successful and failed validation histories equal.'}


def compare_diagnostic(raw, strict, raw_formal_path):
    require(raw['accepted'] is True and raw['errors'] == [] and raw['mode'] == 'raw-only'
            and raw['strict_identity_verified'] is False and raw['actual_binary_verified'] is False
            and raw['recorded_build_identity_verified'] is True, 'diagnostic replay identity mode differs')
    require(raw['formal_audit']['mode'] == 'raw-only' and raw['formal_audit']['sha256'] == sha(raw_formal_path),
            'diagnostic is not bound to freshly recomputed formal audit')
    left, right = copy(raw), copy(strict)
    for value in (left, right):
        for field in ('mode', 'strict_identity_verified', 'actual_binary_verified'):
            value.pop(field)
        for receipt in value['build_receipts'].values():
            receipt.pop('file')
        for field in ('file', 'sha256', 'mode'):
            value['formal_audit'].pop(field)
    require(left == right, 'diagnostic complete arithmetic/retained-identity records differ')
    return {'counts': {'columns': 4, 'wal': 4},
            'comparison': 'All raw/logical-column/WAL/command data equal; only explicit relocated paths and formal report identity mode/hash differ.'}


def compare_summary(raw, strict, formal_path, raw_markdown, strict_markdown):
    require(raw['accepted'] is True and raw['current_audit_sha256'] == sha(formal_path), 'summary does not use fresh formal replay')
    left, right = copy(raw), copy(strict)
    left.pop('current_audit_sha256'); right.pop('current_audit_sha256')
    require(left == right, 'summary numerical/adverse/missing/reference records differ')
    require(raw_markdown.read_bytes() == strict_markdown.read_bytes(), 'rendered report table bytes differ')
    return {'comparison': 'Entire numerical report and rendered Markdown equal; only the freshly recomputed formal audit hash differs.'}


def replay_command(summary, manifest, label):
    require(summary['replay_commands'] == manifest['replay_commands'] == COMMANDS, 'replay commands are not the allowlisted read-only commands')
    return COMMANDS[label].copy()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--evidence', type=Path, default=DEFAULT_EVIDENCE)
    parser.add_argument('--work-dir', type=Path, default=DEFAULT_WORK)
    parser.add_argument('--expect-archive-sha256')
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    evidence, work = args.evidence.resolve(), args.work_dir.resolve()
    output = (args.output or evidence / 'portable-verification.json').resolve()
    require(not work.exists() and not work.is_relative_to(evidence) and not evidence.is_relative_to(work), 'verification work directory must be fresh and separate from evidence')
    require(not output.exists() and not output.is_relative_to(work / 'raw'), 'output must be new and outside extracted raw')
    work.mkdir(parents=True, mode=0o700)
    report = {'accepted': False, 'started_utc': now(), 'errors': [], 'commands': [],
              'verification_script_sha256': sha(__file__), 'actual_host_binaries_reverified': False,
              'actual_host_sources_reverified': False, 'external_verification_directory': str(work)}
    try:
        summary_path = evidence / 'summary.json'
        summary_before = fingerprint(summary_path)
        summary = load(summary_path)
        require(summary['format'] == FORMAT and summary['archive'] == 'raw.tar.xz', 'unexpected archive summary')
        for field in ('raw_files', 'raw_bytes', 'manifest_bytes', 'archive_bytes'):
            integer(summary[field], field)
        for field in ('archive_sha256', 'manifest_sha256'):
            require(re.fullmatch('[0-9a-f]{64}', summary[field]), 'malformed summary hash')
        require(summary['raw_files'] > 0 and summary['manifest_bytes'] > 0, 'empty archive')
        archive = evidence / summary['archive']
        archive_before = fingerprint(archive)
        digest = sha(archive)
        require(digest == summary['archive_sha256'] and archive.stat().st_size == summary['archive_bytes'], 'archive hash/size differs from summary')
        if args.expect_archive_sha256:
            require(digest == args.expect_archive_sha256, 'archive differs from independently expected SHA')
        require(shutil.disk_usage(work).free > 2 * (summary['raw_bytes'] + summary['manifest_bytes']), 'insufficient free space')
        root = work / 'raw'
        manifest, expected = extract_and_verify(archive, summary, root)
        require(sha(root / 'verify-archive.py') == report['verification_script_sha256'], 'archive contains a different verification script')
        report.update(archive_sha256=digest, archive_bytes=summary['archive_bytes'], summary_sha256=sha(summary_path),
                      manifest_sha256=summary['manifest_sha256'], verified_payload_files=summary['raw_files'],
                      verified_payload_bytes=summary['raw_bytes'], all_payload_stream_hashes_match=True, no_extra_members=True)
        report['phase_and_reference_binding'] = verify_scope(root, summary)
        strict_formal, strict_diagnostic, strict_summary = strict_reports(root)
        require(summary['strict_report_sha256'] == {name: sha(root / name) for name in ('direct-audit.json', 'diagnostic-audit-final.json', 'report-data.json', 'report-tables.md', 'replication-audit-final.json')}, 'strict report hashes differ')
        environment = dict(os.environ, PYTHONDONTWRITEBYTECODE='1')
        environment.pop('PYTHONPATH', None); environment.pop('PYTHONHOME', None)
        for label in ('replication', 'formal', 'diagnostic', 'summary'):
            command = replay_command(summary, manifest, label)
            record = {'label': label, 'argv': command, 'cwd': str(root), 'start_utc': now()}
            report['commands'].append(record)
            with (work / (label + '.stdout')).open('x') as stdout, (work / (label + '.stderr')).open('x') as stderr:
                result = subprocess.run(command, cwd=root, env=environment, stdout=stdout, stderr=stderr)
            record.update(end_utc=now(), exit=result.returncode, stdout_sha256=sha(work / (label + '.stdout')), stderr_sha256=sha(work / (label + '.stderr')))
            require(result.returncode == 0, label + ' replay failed; see external retained logs')
        replication = load(root / 'replication-audit-final.json')
        replay = load(work / 'replication-raw-audit.json')
        require(replication['accepted'] is True and replication['raw_only'] is False
                and replay['accepted'] is True and replay['raw_only'] is True and not replay['errors'], 'replication replay rejected')
        require({k:v for k,v in replication.items() if k != 'raw_only'} == {k:v for k,v in replay.items() if k != 'raw_only'}, 'replication raw records differ')
        require(len(replay['products']) == 24 and len(replay['comparisons']) == 12, 'replication coverage differs')
        report['exploratory_stream_replication_identical'] = True
        formal_path = work / 'direct-raw-audit.json'
        report['formal_numerics_and_admissions_identical'] = compare_formal(load(formal_path), strict_formal)
        report['diagnostic_numerics_and_identity_records_identical'] = compare_diagnostic(load(work / 'diagnostic-raw-audit.json'), strict_diagnostic, formal_path)
        report['summary_numerics_and_tables_identical'] = compare_summary(load(work / 'report-data.json'), strict_summary, formal_path, work / 'report-tables.md', root / 'report-tables.md')
        report['replayed_report_sha256'] = {name: sha(work / name) for name in ('direct-raw-audit.json', 'diagnostic-raw-audit.json', 'report-data.json', 'report-tables.md', 'replication-raw-audit.json')}
        verify_tree(root, expected)
        require(fingerprint(summary_path) == summary_before and fingerprint(archive) == archive_before, 'archive/summary changed during verification')
        report['post_audit_raw_inventory_and_hashes_unchanged'] = True
        report['accepted'] = True
    except Exception as error:
        report['errors'].append(f'{type(error).__name__}: {error}')
    report['finished_utc'] = now()
    report['scope'] = 'Safe complete archive extraction, per-file integrity, retained raw-only arithmetic and tables. No products/builds/profiler sampling or historical machine identity checks. Fixed prior Pi/DeepSeek are not rerun or re-derived from their separate old raw archive.'
    encoded = json.dumps(report, indent=2, allow_nan=False) + '\n'
    (work / 'verification-result.json').write_text(encoded)
    with output.open('x') as stream:
        stream.write(encoded)
    print(json.dumps({'accepted': report['accepted'], 'errors': report['errors'], 'output': str(output)}, indent=2))
    return int(not report['accepted'])


if __name__ == '__main__':
    raise SystemExit(main())
