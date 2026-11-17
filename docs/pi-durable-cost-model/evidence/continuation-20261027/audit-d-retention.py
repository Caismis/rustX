#!/usr/bin/env python3
"""Read retained data only; no product executions, builds, or new timings."""
import copy
import hashlib
import json
import math
from pathlib import Path
import statistics
import subprocess
import sys

BASE = Path('/home/capi/.cache/rustx-pr485-next')
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
    fields = point['process']['stat'].rsplit(')', 1)[1].split()
    return int(fields[11]) + int(fields[12])

def writes(point):
    return int(dict(line.split(':', 1) for line in point['process']['io'].splitlines())['write_bytes'])

def memory(point, field):
    return int(dict(line.split(':', 1) for line in point['process']['smaps_rollup'].splitlines() if ':' in line)[field].split()[0]) / 1024

def metrics(raw, ticks):
    lifecycle = {p['phase']: p for p in raw['lifecycle']}
    measured = [p for p in raw['samples'] if p['measured']]
    start = raw['samples'][raw['history'] - 1] if raw['history'] else lifecycle['after_initialization']
    end, final = raw['samples'][-1], lifecycle['after_cleanup']
    points = raw['samples'] + raw['lifecycle']
    return {'mean_turn_ms': statistics.mean(p['settled_us'] for p in measured) / 1000,
            'turn_cpu_ms_per_turn': (cpu_ticks(end) - cpu_ticks(start)) * 1000 / ticks / len(measured),
            'through_cleanup_self_cpu_ms': cpu_ticks(final) * 1000 / ticks,
            'through_cleanup_process_write_mib': writes(final) / 2**20,
            'lifecycle_ms': (final['elapsed_us'] - raw['lifecycle'][0]['elapsed_us']) / 1000,
            'max_sampled_anon_pss_mib': max(memory(p, 'Pss_Anon') for p in points),
            'max_sampled_pss_mib': max(memory(p, 'Pss') for p in points),
            'max_sampled_rss_mib': max(memory(p, 'Rss') for p in points)}

def assert_close(actual, expected):
    assert math.isclose(actual, expected, rel_tol=1e-11, abs_tol=1e-10), (actual, expected)

recorded = json.loads((BASE / 'direct-d-recomputed.json').read_text())
recorded_products = {p['file']: p for p in recorded['products']}
identities = {}
for label, directory in [('before', Path('/home/capi/.cache/rustx-pr485-evidence/candidate-final-build')), ('after', BASE / 'candidate-d-build')]:
    identity = json.loads((directory / 'identity.json').read_text())
    assert identity['sha256'] == sha(identity['binary'])
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
            assert artifact['profile']['opt_level'] == '3'
            assert artifact['profile']['debuginfo'] == 0 and not artifact['profile']['debug_assertions']
            assert artifact['features'] == [] and artifact['fresh'] is False
            root_builds.append({'target': artifact['target']['name'], 'kind': artifact['target']['kind'], 'profile': artifact['profile']})
    assert len(root_builds) == 3
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
    assert len(list(folder.glob('*.process.json'))) == 36
    assert len(list(folder.glob('*-comparison.json'))) == 18
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
                sample_metrics[label] = metric
                rel = str(raw_path.relative_to(BASE))
                previous = recorded_products[rel]
                assert sha(raw_path) == previous['raw_sha256'] and sha(proc_path) == previous['process_sha256']
                for key, value in metric.items():
                    assert_close(value, previous['metrics'][key])
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
                pairs.append({'cohort': cohort, 'mode': mode, 'pair': pair, 'case': case, 'metric': key, 'before': before, 'after': after, 'delta': after - before, 'ratio': after / before})

for cohort, mode in MODES.items():
    for case in CASES:
        for metric in products[0]['metrics']:
            selected = [r for r in pairs if r['cohort'] == cohort and r['case'] == case and r['metric'] == metric]
            ratios = [r['ratio'] for r in selected]
            logs = [math.log(r) for r in ratios]
            middle = statistics.mean(logs)
            radius = math.sqrt(2 * .9**2 / (1 - .9**2)) * statistics.stdev(logs) / math.sqrt(3)
            summary = {'cohort': cohort, 'mode': mode, 'case': case, 'metric': metric, 'ratios': ratios, 'absolute_deltas': [r['delta'] for r in selected], 'median_ratio': statistics.median(ratios), 'minimum': min(ratios), 'maximum': max(ratios), 'lower_count': sum(r < 1 for r in ratios), 'higher_count': sum(r > 1 for r in ratios), 'equal_count': sum(r == 1 for r in ratios), 'geometric_mean': math.exp(middle), 'lower90': math.exp(middle - radius), 'upper90_one_sided95': math.exp(middle + radius)}
            old = next(r for r in recorded['summaries'] if (r['cohort'], r['case'], r['metric']) == (cohort, case, metric))
            assert old['n'] == 3 and old['values'] == ratios
            for key in ('minimum', 'maximum', 'geometric_mean', 'lower90', 'upper90_one_sided95'):
                assert_close(summary[key], old[key])
            assert_close(summary['median_ratio'], old['median'])
            groups.append(summary)

assert len(products) == recorded['accepted_products'] == 72
assert paired_requests == recorded['request_comparisons'] == 846
assert requests_checked == 1692
audit = {'verified': True, 'audit_script_sha256': sha(__file__), 'predeclaration_sha256': sha(BASE / 'experiment.txt'), 'reference_recomputation_sha256': sha(BASE / 'direct-d-recomputed.json'), 'identities': identities, 'products_checked': len(products), 'independently_checked_wire_requests': requests_checked, 'paired_equal_requests': paired_requests, 'all_recorded_metrics_and_summaries_recomputed': True, 'products': products, 'paired_results': pairs, 'groups': groups, 'pair_evidence': evidence, 'decision': 'Retention defensible with explicitly retained adverse observations; no overall parity or nonregression conclusion. Candidate is one jointly measured change set.', 'limits': ['No new executions, builds, profiles, or timing samples.', 'Build and source identities verified; external-machine scheduling interference cannot be reconstructed from these files.', 'Controller correctness validators rerun from retained raw data; independent expected wire history additionally checked.', 'Only three alternating pairs; reported Student-t log-ratio intervals depend on unverified distribution/independence assumptions and are not parity evidence.', 'Self CPU is quantized at 10ms; complete CPU is absolute through cleanup, including startup, harness, and verification.', 'Writes are process-attributed and memory is maximum sampled PSS/RSS, not device I/O or peak allocation.', 'No timing benefit is attributed separately to counting or readiness coalescing.']}
(BASE / 'd-retention-audit.json').write_text(json.dumps(audit, indent=2) + '\n')
print(json.dumps({'verified': True, 'products': len(products), 'wire_requests': requests_checked, 'paired_requests': paired_requests, 'groups': len(groups)}))
