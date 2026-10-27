#!/usr/bin/env python3
"""Independently audit E versus D retained raw evidence; never execute products.

Run only after the parent declares candidate-e-direct measurements complete.
No retention decision is predetermined. All adverse pairs remain in the output.
"""
import argparse
import copy
import hashlib
import json
import math
from pathlib import Path
import statistics
import subprocess
import sys

CACHE = Path('/home/capi/.cache/rustx-pr485-next')
EXPECTED = {
    'before': {'head': 'e8d6380fa6ea0724b752970473d093c629cb8a63', 'tree': 'd6d193e07b16db0ab9b1ce646a254a1c1674c80e', 'sha256': '8ec26a4a08bde085f46ee40159d3870f7ead675bec39f42cb4f3e55df1d1d233'},
    'after': {'head': 'a39f343b54a46c8e5527dad88842cb8a6d46691b', 'tree': '7f4952a1e2473ce1a97e2622c742789775a685f7', 'sha256': '19c52cb8b539abfc0a259d6493ad08b65df00a1e4cc782284cdf562ffa866cf7'},
}
REPO = Path('/home/capi/.codex/worktrees/pr485-pi-performance/rustX')
sys.path.insert(0, str(REPO / 'benchmarks/runtime'))
from controller import validate_sample

CASES = ['short-small', 'short-large', 'stream', 'large', 'tool', 'cancel']
MODES = {'direct-no-live': 'no-live-subscriber', 'direct-live': 'live-subscriber'}
COUNTS = {'short-small': 40, 'short-large': 96, 'stream': 1, 'large': 1, 'tool': 2, 'cancel': 1}
FIXTURE = json.loads((REPO / 'tests/conformance/agent_loop/runtime_benchmark/expected_tools.json').read_text())

def sha(path):
    with Path(path).open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()

def git(*args):
    return subprocess.check_output(['git', '-C', str(REPO), *args])

def text_message(role, value):
    return {'role': role, 'content': [{'type': 'text', 'text': value}], 'tool_calls': None}

def expected_body(case, ordinal):
    messages = [text_message('user', 'benchmark turn 0000'), text_message('user', '<system-reminder>\nTimezone: UTC\nCurrent time: 2026-10-19 00:00:00\n</system-reminder>')]
    if case.startswith('short-'):
        for i in range(ordinal):
            messages += [text_message('assistant', f'reply {i:04d}'), text_message('user', f'benchmark turn {i + 1:04d}')]
    elif case == 'tool' and ordinal == 1:
        messages += [{'role': 'assistant', 'content': [], 'tool_calls': [{'type': 'function', 'id': 'benchmark-read', 'function': {'name': 'read', 'arguments': {'path': '<WORKSPACE>/note.txt'}}}]}, dict(text_message('tool', 'deterministic-note-payload-6d41'), tool_call_id='benchmark-read')]
    else:
        assert ordinal == 0
    return {'model': 'chat-model', 'messages': messages, 'stream': True, 'stream_options': {'include_usage': True}, 'max_completion_tokens': 1024, 'tools': FIXTURE}

def normalize(body, workspace):
    body = copy.deepcopy(body)
    for message in body['messages']:
        if isinstance(message.get('content'), str):
            message['content'] = [{'type': 'text', 'text': message['content']}]
        elif message.get('content') is None:
            message['content'] = []
        message.setdefault('tool_calls', None)
        for call in message['tool_calls'] or []:
            args = call['function']['arguments']
            if isinstance(args, str):
                args = json.loads(args)
            if args.get('path') == workspace + '/note.txt':
                args['path'] = '<WORKSPACE>/note.txt'
            call['function']['arguments'] = args
    return body

def cpu_ticks(point):
    if point['process']['stat'] is None:
        return None
    fields = point['process']['stat'].rsplit(')', 1)[1].split()
    return int(fields[11]) + int(fields[12])

def writes(point):
    entries = dict(line.split(':', 1) for line in (point['process']['io'] or '').splitlines())
    return int(entries['write_bytes']) if 'write_bytes' in entries else None

def memory(point, field):
    entries = dict(line.split(':', 1) for line in (point['process']['smaps_rollup'] or '').splitlines() if ':' in line)
    return int(entries[field].split()[0]) / 1024 if field in entries else None

def scale(value, factor):
    return value * factor if value is not None else None

def delta(after, before):
    return after - before if after is not None and before is not None else None

def complete_max(values):
    return max(values) if all(value is not None for value in values) else None

def metrics(raw, ticks):
    lifecycle = {p['phase']: p for p in raw['lifecycle']}
    measured = [p for p in raw['samples'] if p['measured']]
    start = raw['samples'][raw['history'] - 1] if raw['history'] else lifecycle['after_initialization']
    end, final = raw['samples'][-1], lifecycle['after_cleanup']
    points = raw['samples'] + raw['lifecycle']
    return {'mean_turn_ms': statistics.mean(p['settled_us'] for p in measured) / 1000,
            'turn_cpu_ms_per_turn': scale(delta(cpu_ticks(end), cpu_ticks(start)), 1000 / ticks / len(measured)),
            'through_cleanup_self_cpu_ms': scale(cpu_ticks(final), 1000 / ticks),
            'through_cleanup_process_write_mib': scale(writes(final), 1 / 2**20),
            'lifecycle_ms': (final['elapsed_us'] - raw['lifecycle'][0]['elapsed_us']) / 1000,
            'max_sampled_anon_pss_mib': complete_max([memory(p, 'Pss_Anon') for p in points]),
            'max_sampled_pss_mib': complete_max([memory(p, 'Pss') for p in points]),
            'max_sampled_rss_mib': complete_max([memory(p, 'Rss') for p in points])}

def assert_close(actual, expected):
    assert actual == expected if actual is None or expected is None else math.isclose(actual, expected, rel_tol=1e-11, abs_tol=1e-10), (actual, expected)

def paired_ratio(before, after):
    if before is None or after is None:
        return None, 'missing_counter'
    if before == 0:
        return None, 'zero_denominator'
    return after / before, None

def audit(args):
    BASE = args.root
    reference = args.reference or BASE / 'recomputed.json'
    recorded = json.loads(reference.read_text())
    reference_differences = []
    def compare_reference(actual, expected, context):
        try:
            assert_close(actual, expected)
        except AssertionError:
            reference_differences.append({'context': context, 'independent': actual, 'reference': expected})
    recorded_products = {p['file']: p for p in recorded['products']}
    identities = {}
    for label, directory in [('before', CACHE / 'candidate-d-build'), ('after', CACHE / 'candidate-e-build')]:
        identity = json.loads((directory / 'identity.json').read_text())
        assert identity['head'] == EXPECTED[label]['head'], (label, 'unexpected source head')
        assert identity['production_tree'] == EXPECTED[label]['tree'], (label, 'unexpected source tree')
        assert identity['sha256'] == EXPECTED[label]['sha256'] == sha(identity['binary']), (label, 'unexpected actual/recorded binary identity')
        assert identity['production_tree'] == git('rev-parse', identity['head'] + ':src').decode().strip()
        assert identity['verified'] and identity['clean_exit'] == identity['build_exit'] == 0
        for path, digest in identity['harness'].items():
            assert hashlib.sha256(git('show', identity['head'] + ':' + path)).hexdigest() == digest
        root_builds = []
        for line in (directory / 'build.stdout').read_text().splitlines():
            try:
                artifact = json.loads(line)
            except json.JSONDecodeError:
                continue
            if artifact.get('reason') == 'compiler-artifact' and artifact['target']['name'] in ('rustx', 'conformance'):
                # E also built a same-binary Reader diagnostic libtest. The
                # runtime conformance executable links the non-cfg(test) library.
                if artifact['target']['kind'] == ['lib'] and artifact['profile']['test']:
                    continue
                assert artifact['profile']['opt_level'] == '3'
                assert artifact['profile']['debuginfo'] == 0 and not artifact['profile']['debug_assertions']
                assert artifact['features'] == [] and artifact['fresh'] is False
                root_builds.append({'target': artifact['target']['name'], 'kind': artifact['target']['kind'], 'profile': artifact['profile']})
        assert len(root_builds) == 3
        assert sorted((r['target'], tuple(r['kind'])) for r in root_builds) == [('conformance', ('test',)), ('rustx', ('bin',)), ('rustx', ('lib',))]
        identities[label] = {'identity_sha256': sha(directory / 'identity.json'), 'source_head': identity['head'], 'production_tree': identity['production_tree'], 'binary': identity['binary'], 'binary_sha256': identity['sha256'], 'rustc': identity['rustc'], 'harness': identity['harness'], 'root_builds': root_builds}
    assert identities['before']['harness'] == identities['after']['harness']
    assert identities['before']['rustc'] == identities['after']['rustc']

    products, pairs, groups, evidence = [], [], [], []
    requests_checked = paired_requests = 0
    for cohort, mode in MODES.items():
        folder = BASE / cohort
        manifest = json.loads((folder / 'manifest.json').read_text())
        assert manifest['observer_mode'] == mode and manifest['cases'] == CASES
        assert manifest['pairs'] == 3 and manifest['pair_offset'] == 0 and manifest['clock_ticks'] == 100
        expected_stems = {f'{pair}-{case}-{engine}' for pair in range(3) for case in CASES for engine in ('before', 'after')}
        assert {p.name for p in folder.glob('*.process.json')} == {s + '.process.json' for s in expected_stems}
        assert {p.name for engine in ('before', 'after') for p in folder.glob('*-' + engine + '.json')} == {s + '.json' for s in expected_stems}
        assert {p.name for p in folder.glob('*-comparison.json')} == {f'{pair}-{case}-comparison.json' for pair in range(3) for case in CASES}
        assert not list(folder.glob('*error*'))
        for label in ('before', 'after'):
            assert manifest['executables'][label]['sha256'] == identities[label]['binary_sha256']
            assert manifest['executables'][label]['path'] == identities[label]['binary']
        for pair in range(3):
            for case in CASES:
                sample_metrics, raw_bodies = {}, {}
                for label in ('before', 'after'):
                    stem = folder / f'{pair}-{case}-{label}'
                    raw_path, proc_path = stem.with_suffix('.json'), stem.with_suffix('.process.json')
                    raw, process = json.loads(raw_path.read_text()), json.loads(proc_path.read_text())
                    assert process['accepted'] and process['argv'][0] == identities[label]['binary']
                    assert process['observer_mode'] == mode and raw['observer_mode'] == mode
                    validate_sample(process, raw, case, stem.with_suffix('.log').read_text())
                    assert len(raw['wire_requests']) == COUNTS[case]
                    bodies = []
                    for ordinal, wire in enumerate(raw['wire_requests']):
                        body = normalize(wire['body'], process['workspace'])
                        assert body == expected_body(case, ordinal), (cohort, pair, case, label, ordinal)
                        bodies.append(body)
                    raw_bodies[label] = bodies
                    requests_checked += len(bodies)
                    metric = metrics(raw, manifest['clock_ticks'])
                    assert all(value is None or value >= 0 for value in metric.values()), 'negative or decreasing raw counter'
                    sample_metrics[label] = metric
                    rel = str(raw_path.relative_to(BASE))
                    previous = recorded_products[rel]
                    assert sha(raw_path) == previous['raw_sha256'] and sha(proc_path) == previous['process_sha256']
                    for key, value in metric.items():
                        compare_reference(value, previous['metrics'][key], rel + '/' + key)
                    products.append({'file': rel, 'cohort': cohort, 'mode': mode, 'pair': pair, 'case': case, 'engine': label, 'raw_sha256': sha(raw_path), 'process_sha256': sha(proc_path), 'metrics': metric})
                assert raw_bodies['before'] == raw_bodies['after']
                comparison_path = folder / f'{pair}-{case}-comparison.json'
                comparison = json.loads(comparison_path.read_text())
                assert comparison['observer_mode'] == mode and comparison['case'] == case
                assert len(comparison['checked']) == COUNTS[case]
                assert all(c['coreSemanticEqual'] and c['fullSemanticRequestEqual'] for c in comparison['checked'])
                paired_requests += COUNTS[case]
                order = ['before', 'after'] if pair % 2 == 0 else ['after', 'before']
                times = [((folder / f'{pair}-{case}-{label}').with_suffix('.process.json')).stat().st_mtime_ns for label in order]
                assert times == sorted(times), (cohort, pair, case, times)
                evidence.append({'comparison': str(comparison_path.relative_to(BASE)), 'sha256': sha(comparison_path), 'expected_order': order, 'process_result_mtime_ns': times})
                for key in sample_metrics['before']:
                    before, after = sample_metrics['before'][key], sample_metrics['after'][key]
                    ratio, reason = paired_ratio(before, after)
                    pairs.append({'cohort': cohort, 'mode': mode, 'pair': pair, 'case': case, 'metric': key, 'before': before, 'after': after, 'delta': delta(after, before), 'ratio': ratio, 'ratio_unavailable_reason': reason})

    for cohort, mode in MODES.items():
        for case in CASES:
            for metric in products[0]['metrics']:
                selected = [r for r in pairs if r['cohort'] == cohort and r['case'] == case and r['metric'] == metric]
                ratios = [r['ratio'] for r in selected]
                available = [r for r in ratios if r is not None]
                summary = {'cohort': cohort, 'mode': mode, 'case': case, 'metric': metric, 'ratios': ratios, 'absolute_deltas': [r['delta'] for r in selected], 'available': len(available), 'required': 3, 'median_ratio': statistics.median(available) if available else None, 'minimum': min(available) if available else None, 'maximum': max(available) if available else None, 'lower_count': sum(r < 1 for r in available), 'higher_count': sum(r > 1 for r in available), 'equal_count': sum(r == 1 for r in available)}
                if len(available) == 3 and all(r > 0 for r in available):
                    logs = [math.log(r) for r in available]
                    middle = statistics.mean(logs)
                    radius = math.sqrt(2 * .9**2 / (1 - .9**2)) * statistics.stdev(logs) / math.sqrt(3)
                    summary.update(geometric_mean=math.exp(middle), lower90=math.exp(middle - radius), upper90_one_sided95=math.exp(middle + radius))
                else:
                    summary['log_interval_unavailable_reason'] = 'missing ratio, zero denominator, or zero numerator; all original positions and zero values retained'
                old = next(r for r in recorded['summaries'] if (r['cohort'], r['case'], r['metric']) == (cohort, case, metric))
                assert old['n'] == 3
                for pair, (actual, previous) in enumerate(zip(ratios, old['values'], strict=True)):
                    compare_reference(actual, previous, f'{cohort}/{case}/{metric}/pair{pair}')
                for key in ('minimum', 'maximum', 'geometric_mean', 'lower90', 'upper90_one_sided95'):
                    if key in old and key in summary:
                        compare_reference(summary[key], old[key], f'{cohort}/{case}/{metric}/{key}')
                if 'median' in old:
                    compare_reference(summary['median_ratio'], old['median'], f'{cohort}/{case}/{metric}/median')
                groups.append(summary)

    assert len(products) == recorded['accepted_products'] == 72
    assert paired_requests == recorded['request_comparisons'] == 846
    assert requests_checked == 1692
    audit = {'verified': not reference_differences, 'reference_differences': reference_differences, 'audit_script_sha256': sha(__file__), 'predeclaration': {'path': str(args.predeclaration), 'sha256': sha(args.predeclaration), 'text': args.predeclaration.read_text()}, 'reference_recomputation_sha256': sha(reference), 'identities': identities, 'products_checked': len(products), 'independently_checked_wire_requests': requests_checked, 'paired_equal_requests': paired_requests, 'all_recorded_metrics_and_summaries_recomputed': not reference_differences, 'products': products, 'paired_results': pairs, 'groups': groups, 'pair_evidence': evidence, 'decision': 'requires_review: evidence audit does not predetermine retention; assess runtime outcomes together with correctness and the separate Reader tradeoff diagnostic', 'all_adverse_pairs': [row for row in pairs if row['ratio'] is not None and row['ratio'] > 1], 'groups_higher_in_all_three_pairs': [row for row in groups if row['higher_count'] == 3], 'groups_lower_in_all_three_pairs': [row for row in groups if row['lower_count'] == 3], 'limits': ['No new executions, builds, profiles, or timing samples.', 'Build and source identities verified; external-machine scheduling interference cannot be reconstructed from these files.', 'Controller correctness validators rerun from retained raw data; independent expected wire history additionally checked.', 'Only three alternating pairs; reported Student-t log-ratio intervals depend on unverified distribution/independence assumptions and are not parity evidence.', 'Self CPU is quantized at 10ms; complete CPU is absolute through cleanup, including startup, harness, and verification.', 'Writes are process-attributed and memory is maximum sampled PSS/RSS, not device I/O or peak allocation.', 'This is E versus retained D. D already contains counting plus readiness coalescing; do not reassign those changes an E timing benefit.', 'The ordinary runtime lifecycle does not cover all explicit scalar field/page reads. This audit alone cannot establish acceptable Reader latency or make the retention decision.', 'The copied optimized diagnostic libtest is excluded from production artifact counting. Reader timings use that cfg(test) binary and are not production B/E end-to-end measurements.']}
    args.output.write_text(json.dumps(audit, indent=2) + '\n')
    print(json.dumps({'verified': not reference_differences, 'products': len(products), 'wire_requests': requests_checked, 'paired_requests': paired_requests, 'groups': len(groups), 'reference_differences': reference_differences}))
    return 0 if not reference_differences else 1

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=CACHE / 'candidate-e-direct')
    parser.add_argument('--output', type=Path, default=CACHE / 'e-retention-audit.json')
    parser.add_argument('--reference', type=Path)
    parser.add_argument('--predeclaration', type=Path, default=CACHE / 'reference-policy-addendum.txt')
    raise SystemExit(audit(parser.parse_args()))
