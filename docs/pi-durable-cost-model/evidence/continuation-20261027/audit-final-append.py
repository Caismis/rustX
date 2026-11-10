#!/usr/bin/env python3
"""Independently audit retained append evidence, using only the standard library.

No product, build, profiler, compression, or producer statistics are executed.
Default input is the final cohort directory; --reference-audit is optional and
compares calculations only after they have been independently derived.
Actual binary verification is strict by default. Archived evidence can be
recalculated with --binary-verification off without the original host paths.
Raw arithmetic is still completed when a strict binary check fails.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path
import statistics

HISTORIES = (0, 64, 512, 2048)
PHASES = ('before_initialization', 'after_seed', 'after_appends', 'after_close', 'after_reopen_verification', 'after_cleanup')
TEST = 'agent_loop::runtime_benchmark::measure_canonical_append'

def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()

def json_file(path, errors):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError) as error:
        errors.append(f'{path}: {type(error).__name__}: {error}')
        return None

def check(condition, message, errors):
    if not condition:
        errors.append(message)

def cell(value=None, status='ok', reason=None, **details):
    result = {'value': value, 'status': status}
    if reason:
        result['reason'] = reason
    return result | details

def scaled(source, scale):
    result = dict(source)
    if isinstance(result['value'], (int, float)):
        result['value'] *= scale
    return result

def difference(before, after, scale=1):
    if before['status'] != 'ok' or after['status'] != 'ok':
        return cell(status='unavailable', reason='endpoint counter missing or invalid', before=before, after=after)
    value = after['value'] - before['value']
    if value < 0:
        return cell(value * scale, 'invalid', 'counter decreased', before=before['value'], after=after['value'])
    return cell(value * scale)

def total(cells):
    if any(x['status'] != 'ok' for x in cells):
        return cell(status='unavailable', reason='one or more required operation brackets are missing or invalid', brackets=cells)
    return cell(sum(x['value'] for x in cells))

def counter(process, field):
    if not isinstance(process, dict):
        return cell(status='missing', reason='process sample absent')
    try:
        if field in ('cpu_ticks', 'pid_start'):
            stat = process.get('stat')
            if stat is None:
                return cell(status='missing', reason='stat absent')
            parts = stat.rsplit(')', 1)[1].split()
            value = int(parts[11]) + int(parts[12]) if field == 'cpu_ticks' else (int(stat.split(' ', 1)[0]), int(parts[19]))
        else:
            source = process.get('io' if field == 'write_bytes' else 'smaps_rollup')
            if source is None:
                return cell(status='missing', reason=f'{field} source absent')
            entries = dict(line.split(':', 1) for line in source.splitlines() if ':' in line)
            value = int(entries[field].split()[0])
        if field != 'pid_start' and value < 0:
            return cell(value, 'invalid', 'negative counter')
        return cell(value)
    except (IndexError, KeyError, TypeError, ValueError) as error:
        return cell(status='invalid', reason=f'cannot parse {field}: {error}')

def metrics(raw, process_record, ticks):
    output, phase_rows, operation_rows = {}, [], []
    rows = raw.get('lifecycle', [])
    rows = rows if isinstance(rows, list) else []
    operations = raw.get('operation_counters', [])
    operations = operations if isinstance(operations, list) else []
    by_phase = {row.get('phase'): row for row in rows if isinstance(row, dict)}
    def endpoint(name, field):
        return counter(by_phase.get(name, {}).get('process'), field)
    def cpu_delta(first, last):
        result = difference(endpoint(first, 'cpu_ticks'), endpoint(last, 'cpu_ticks'))
        return scaled(result, 1000 / ticks) if ticks else cell(status='missing', reason='clock_ticks unavailable')
    def write_delta(first, last):
        return difference(endpoint(first, 'write_bytes'), endpoint(last, 'write_bytes'))
    def wall_delta(first, last):
        a, b = by_phase.get(first, {}).get('elapsed_us'), by_phase.get(last, {}).get('elapsed_us')
        if type(a) is not int or type(b) is not int:
            return cell(status='missing', reason='phase elapsed_us unavailable')
        return difference(cell(a), cell(b), .001)
    appends = raw.get('append_ns', [])
    appends = appends if isinstance(appends, list) else []
    valid_wall = len(appends) == 32 and all(type(value) is int and value >= 0 for value in appends)
    output['mean_append_ms'] = cell(statistics.mean(appends) / 1e6) if valid_wall else cell(status='invalid', reason='requires exactly 32 nonnegative integer durations', raw_values=appends)
    output['median_single_append_ms'] = cell(statistics.median(appends) / 1e6) if valid_wall else dict(output['mean_append_ms'])
    output['append_phase_cpu_ms'] = cpu_delta('after_seed', 'after_appends')
    output['append_phase_cpu_ms_per_append'] = scaled(output['append_phase_cpu_ms'], 1 / 32)
    output['append_phase_write_mib'] = scaled(write_delta('after_seed', 'after_appends'), 1 / 2**20)
    output['append_phase_wall_ms'] = wall_delta('after_seed', 'after_appends')
    for before, after in zip(PHASES, PHASES[1:]):
        phase_rows.append({'from': before, 'through': after, 'wall_ms': wall_delta(before, after), 'self_cpu_ms': cpu_delta(before, after), 'process_write_bytes': write_delta(before, after)})
    for ordinal, bracket in enumerate(operations):
        bracket = bracket if isinstance(bracket, dict) else {}
        values = {}
        for field in ('cpu_ticks', 'write_bytes'):
            before, after = counter(bracket.get('before'), field), counter(bracket.get('after'), field)
            values[field] = difference(before, after)
            values[field + '_before'] = before
            values[field + '_after'] = after
        operation_rows.append({'ordinal': ordinal, **values})
    if raw.get('cold') is True:
        if len(operation_rows) == 32 and ticks:
            output['cold_operation_cpu_ms_per_append'] = scaled(total([x['cpu_ticks'] for x in operation_rows]), 1000 / ticks / 32)
            output['cold_operation_write_mib_per_append'] = scaled(total([x['write_bytes'] for x in operation_rows]), 1 / 32 / 2**20)
        else:
            output['cold_operation_cpu_ms_per_append'] = cell(status='missing', reason='requires 32 operation brackets and clock_ticks')
            output['cold_operation_write_mib_per_append'] = cell(status='missing', reason='requires 32 operation brackets')
        output['append_cpu_ms_per_append'] = dict(output['cold_operation_cpu_ms_per_append'])
        output['append_write_mib_per_append'] = dict(output['cold_operation_write_mib_per_append'])
    else:
        output['cold_operation_cpu_ms_per_append'] = cell(status='not_applicable', reason='warm case has only phase counters')
        output['cold_operation_write_mib_per_append'] = cell(status='not_applicable', reason='warm case has only phase counters')
        output['append_cpu_ms_per_append'] = dict(output['append_phase_cpu_ms_per_append'])
        output['append_write_mib_per_append'] = scaled(output['append_phase_write_mib'], 1 / 32)
    output['lifecycle_cpu_ms'] = cpu_delta(PHASES[0], PHASES[-1])
    output['lifecycle_write_mib'] = scaled(write_delta(PHASES[0], PHASES[-1]), 1 / 2**20)
    output['lifecycle_ms'] = wall_delta(PHASES[0], PHASES[-1])
    output['through_cleanup_self_cpu_ms'] = scaled(endpoint('after_cleanup', 'cpu_ticks'), 1000 / ticks) if ticks else cell(status='missing', reason='clock_ticks unavailable')
    output['through_cleanup_process_write_mib'] = scaled(endpoint('after_cleanup', 'write_bytes'), 1 / 2**20)
    process_ns = process_record.get('process_lifecycle_ns')
    output['process_lifecycle_ms'] = cell(process_ns / 1e6) if type(process_ns) is int and process_ns >= 0 else cell(status='missing', reason='process_lifecycle_ns absent or invalid')
    for name, field in [('rss', 'Rss'), ('pss', 'Pss'), ('anon_pss', 'Pss_Anon')]:
        samples = [counter(row.get('process') if isinstance(row, dict) else None, field) for row in rows]
        # This matches the retained legacy summary scope, while a second metric
        # explicitly includes the extra cold per-operation memory observations.
        all_samples = samples + [counter(bracket.get(side) if isinstance(bracket, dict) else None, field) for bracket in operations for side in ('before', 'after')]
        for key, values in [(f'max_sampled_{name}_mib', samples), (f'max_all_observed_{name}_mib', all_samples)]:
            present = [x['value'] for x in values if x['status'] == 'ok']
            output[key] = cell(max(present) / 1024 if present else None, 'ok' if present and len(present) == len(values) else 'partial', 'maximum captured samples, not peak' if present else 'no valid memory samples', required_samples=len(values), available_samples=len(present), samples=values)
    return output, phase_rows, operation_rows

def distribution(entries):
    values = [entry['value'] if entry['status'] == 'ok' else None for entry in entries]
    available = [value for value in values if value is not None]
    return {'values': values, 'states': [entry['status'] for entry in entries], 'available': len(available), 'required': len(entries), 'zero_values': sum(value == 0 for value in available), 'median': statistics.median(available) if available else None, 'minimum': min(available) if available else None, 'maximum': max(available) if available else None, 'complete': len(available) == len(entries)}

def ratio(before, after):
    if before['status'] != 'ok' or after['status'] != 'ok':
        return cell(status='unavailable', reason='paired endpoint is not valid', before=before, after=after)
    if before['value'] == 0:
        return cell(status='zero_denominator', reason='0/0 and nonzero/0 are undefined; neither substituted with one nor dropped', before=before['value'], after=after['value'])
    return cell(after['value'] / before['value'], before=before['value'], after=after['value'])

def audit(args):
    root = args.root.resolve()
    errors, identity_errors, binary_errors = [], [], []
    manifests, identities, products, pairs, summaries = {}, {}, [], [], []
    strict_binaries = args.binary_verification == 'strict'
    expected_ids = {'before': args.expect_before_sha256, 'after': args.expect_after_sha256}
    for mode in ('warm', 'cold'):
        path = root / f'append-{mode}' / 'manifest.json'
        manifest = json_file(path, errors)
        if not isinstance(manifest, dict):
            continue
        manifests[mode] = {'path': str(path), 'sha256': digest(path), 'data': manifest}
        check(manifest.get('cold') == (mode == 'cold'), f'{mode}: wrong cold mode', errors)
        check(manifest.get('pairs') == 3 and manifest.get('pair_offset') == 0, f'{mode}: expected exactly pairs 0,1,2', errors)
        ticks = manifest.get('clock_ticks')
        check(type(ticks) is int and ticks > 0, f'{mode}: invalid clock_ticks', errors)
        for engine in ('before', 'after'):
            executables = manifest.get('executables')
            advertised = executables.get(engine, {}) if isinstance(executables, dict) else {}
            advertised = advertised if isinstance(advertised, dict) else {}
            binary_name, advertised_sha = advertised.get('path'), advertised.get('sha256')
            valid_name = isinstance(binary_name, str) and bool(binary_name)
            valid_sha = isinstance(advertised_sha, str) and len(advertised_sha) == 64 and all(c in '0123456789abcdef' for c in advertised_sha)
            check(valid_name and valid_sha, f'{mode}/{engine}: invalid advertised executable identity', identity_errors)
            if expected_ids[engine]:
                check(advertised_sha == expected_ids[engine], f'{mode}/{engine}: advertised binary does not match explicitly pinned identity', identity_errors)
            binary = Path(binary_name) if valid_name else None
            if engine not in identities:
                supplied = getattr(args, engine + '_identity')
                identity_path = supplied.resolve() if supplied else binary.parent / 'identity.json' if binary else None
                identity, local_identity_errors = None, []
                # Archive mode does not consult inferred host paths. An
                # explicitly supplied archived build identity is checked.
                if identity_path and (strict_binaries or supplied):
                    identity = json_file(identity_path, local_identity_errors)
                    if isinstance(identity, dict):
                        check(identity.get('sha256') == advertised_sha, f'{engine}: build identity hash mismatch', local_identity_errors)
                        check(identity.get('binary') == binary_name, f'{engine}: recorded build identity binary path mismatch', local_identity_errors)
                        check(identity.get('verified') is True and identity.get('build_exit') == 0, f'{engine}: build identity not verified/successful', local_identity_errors)
                    elif identity is not None:
                        local_identity_errors.append(f'{engine}: build identity is not an object')
                elif strict_binaries:
                    local_identity_errors.append(f'{engine}: build identity path unavailable')
                identity_errors.extend(local_identity_errors)
                identities[engine] = {'binary': binary_name, 'sha256': advertised_sha, 'identity_path': str(identity_path) if identity_path else None, 'identity_sha256': digest(identity_path) if (strict_binaries or supplied) and identity_path and identity_path.is_file() else None, 'build_identity': identity, 'recorded_build_identity_verified': isinstance(identity, dict) and not local_identity_errors, 'binary_file_checks': []}
            else:
                check(identities[engine]['sha256'] == advertised_sha, f'{engine}: modes advertise different binaries', identity_errors)
            actual_sha, local_binary_errors = None, []
            if strict_binaries:
                try:
                    if binary is None:
                        raise ValueError('executable path absent')
                    actual_sha = digest(binary)
                    check(actual_sha == advertised_sha, f'{mode}/{engine}: actual binary hash mismatch', local_binary_errors)
                except (OSError, ValueError) as error:
                    local_binary_errors.append(f'{mode}/{engine}: actual binary unavailable: {error}')
            binary_errors.extend(local_binary_errors)
            identities[engine]['binary_file_checks'].append({'mode': mode, 'path': binary_name, 'actual_sha256': actual_sha, 'status': 'verified' if strict_binaries and not local_binary_errors else 'failed' if strict_binaries else 'not_attempted', 'errors': local_binary_errors})
        expected_files = {f'{pair}-{history}-{engine}.process.json' for pair in range(3) for history in HISTORIES for engine in ('before', 'after')}
        actual_files = {p.name for p in path.parent.glob('*.process.json')}
        check(actual_files == expected_files, f'{mode}: missing/extra product records: missing={sorted(expected_files - actual_files)}, extra={sorted(actual_files - expected_files)}', errors)
        for pair in range(3):
            for history in HISTORIES:
                for engine in ('before', 'after'):
                    stem = path.parent / f'{pair}-{history}-{engine}'
                    local_errors = []
                    raw = json_file(stem.with_suffix('.json'), local_errors)
                    process = json_file(stem.with_suffix('.process.json'), local_errors)
                    try:
                        log = stem.with_suffix('.log').read_text()
                    except OSError as error:
                        local_errors.append(str(error)); log = ''
                    raw = raw if isinstance(raw, dict) else {}
                    process = process if isinstance(process, dict) else {}
                    expected_test = TEST.replace('measure_canonical_append', 'measure_cold_canonical_append') if mode == 'cold' else TEST
                    check(process.get('accepted') is True and process.get('exit') == 0, 'process not accepted or exit nonzero', local_errors)
                    check(f'test {expected_test} ... ok' in log, 'successful exact test result absent', local_errors)
                    check(raw.get('history') == history and raw.get('measured') == 32 and raw.get('oracle') == 'passed' and raw.get('cold') == (mode == 'cold'), 'history/measured/oracle/mode mismatch', local_errors)
                    appends = raw.get('append_ns')
                    check(isinstance(appends, list) and len(appends) == 32 and all(type(x) is int and x >= 0 for x in appends), 'invalid append durations', local_errors)
                    lifecycle = raw.get('lifecycle')
                    check(isinstance(lifecycle, list), 'lifecycle is absent or not an array', local_errors)
                    lifecycle = lifecycle if isinstance(lifecycle, list) else []
                    check([row.get('phase') if isinstance(row, dict) else None for row in lifecycle] == list(PHASES), 'lifecycle endpoints missing/reordered/duplicated', local_errors)
                    times = [row.get('elapsed_us') if isinstance(row, dict) else None for row in lifecycle]
                    check(all(type(x) is int and x >= 0 for x in times) and times == sorted(times) if all(type(x) is int for x in times) else False, 'lifecycle times invalid/nonmonotonic', local_errors)
                    operations = raw.get('operation_counters')
                    check(isinstance(operations, list), 'operation counters absent or not an array', local_errors)
                    operations = operations if isinstance(operations, list) else []
                    check(len(operations) == (32 if mode == 'cold' else 0), 'incorrect operation bracket count', local_errors)
                    check(all(isinstance(x, dict) and set(x) == {'before', 'after'} for x in operations), 'operation bracket shape differs', local_errors)
                    sampled = [row.get('process') if isinstance(row, dict) else None for row in lifecycle] + [bracket.get(side) if isinstance(bracket, dict) else None for bracket in operations for side in ('before', 'after')]
                    pids = [counter(row, 'pid_start') for row in sampled]
                    known_pids = {tuple(row['value']) for row in pids if row['status'] == 'ok'}
                    check(len(known_pids) <= 1, 'mixed PID/start identities within one product', local_errors)
                    derived, phases, op_rows = metrics(raw, process, ticks if type(ticks) is int and ticks > 0 else None)
                    check(all(row['status'] != 'invalid' for row in derived.values()), 'invalid/decreasing derived metric', local_errors)
                    errors.extend(f'{mode}/{pair}/{history}/{engine}: {message}' for message in local_errors)
                    products.append({'mode': mode, 'pair': pair, 'history': history, 'engine': engine, 'accepted': not local_errors, 'validation_errors': local_errors, 'source_files': {suffix: {'path': str(stem.with_suffix(suffix)), 'sha256': digest(stem.with_suffix(suffix)) if stem.with_suffix(suffix).exists() else None} for suffix in ('.json', '.process.json', '.log')}, 'schema': raw.get('schema'), 'append_ns': appends, 'metrics': derived, 'phase_deltas': phases, 'operation_counters': op_rows, 'pid_start_samples': pids, 'before_close': raw.get('before_close'), 'closed_footprint': raw.get('closed_footprint')})
    if set(manifests) == {'warm', 'cold'}:
        check(manifests['warm']['data'].get('executables') == manifests['cold']['data'].get('executables'), 'warm/cold binary manifests differ', identity_errors)
        check(manifests['warm']['data'].get('clock_ticks') == manifests['cold']['data'].get('clock_ticks'), 'warm/cold clock scales differ', errors)
    if set(identities) == {'before', 'after'}:
        check(identities['before']['sha256'] != identities['after']['sha256'], 'same binary cannot establish a revision comparison', identity_errors)
        left, right = identities['before']['build_identity'], identities['after']['build_identity']
        if isinstance(left, dict) and isinstance(right, dict):
            check(left.get('harness') == right.get('harness'), 'benchmark harness hashes differ between builds', identity_errors)
            check(left.get('rustc') == right.get('rustc'), 'compiler identities differ between builds', identity_errors)
    for mode in ('warm', 'cold'):
        for history in HISTORIES:
            grouped = {pair: {p['engine']: p for p in products if (p['mode'], p['history'], p['pair']) == (mode, history, pair)} for pair in range(3)}
            names = sorted({name for group in grouped.values() for p in group.values() for name in p['metrics']})
            combined = {}
            for name in names:
                before_cells, after_cells, ratios = [], [], []
                for pair, group in grouped.items():
                    def admitted(engine):
                        p = group.get(engine)
                        return p['metrics'][name] if p and p['accepted'] else cell(status='rejected_product', reason='product validation failed or file absent')
                    before, after = admitted('before'), admitted('after')
                    paired = ratio(before, after)
                    before_cells.append(before); after_cells.append(after); ratios.append(paired)
                    pairs.append({'mode': mode, 'history': history, 'pair': pair, 'metric': name, 'before': before, 'after': after, 'after_over_before': paired})
                combined[name] = {'before': distribution(before_cells), 'after': distribution(after_cells), 'after_over_before': distribution(ratios)}
            summaries.append({'mode': mode, 'history': history, 'n_pairs': 3, 'metrics': combined})
    reference = None
    if args.reference_audit:
        reference_errors = []
        old = json_file(args.reference_audit, reference_errors)
        compared = 0
        if isinstance(old, dict):
            for old_summary in old.get('summaries', []):
                new_summary = next((s for s in summaries if (s['mode'], s['history']) == (old_summary['mode'], old_summary['history'])), None)
                if new_summary is None:
                    reference_errors.append('reference group missing'); continue
                for name, old_metric in old_summary['metrics'].items():
                    if name not in new_summary['metrics']:
                        continue
                    for side, old_values in old_metric.items():
                        new_values = new_summary['metrics'][name][side]
                        for key in ('values', 'median', 'minimum', 'maximum'):
                            a, b = old_values.get(key), new_values.get(key)
                            av, bv = (a, b) if isinstance(a, list) else ([a], [b])
                            equal = len(av) == len(bv) and all(x == y if x is None or y is None else math.isclose(x, y, rel_tol=1e-10, abs_tol=1e-10) for x, y in zip(av, bv))
                            if not equal:
                                reference_errors.append(f"{old_summary['mode']}/{old_summary['history']}/{name}/{side}/{key}: {a} != {b}")
                            compared += 1
        reference = {'path': str(args.reference_audit), 'sha256': digest(args.reference_audit) if args.reference_audit.exists() else None, 'checks': compared, 'mismatches': reference_errors}
        errors.extend('reference: ' + e for e in reference_errors)
    check(len(products) == 48, 'expected 48 products: 2 modes × 4 histories × 3 pairs × 2 binaries', errors)
    raw_data_verified = not errors
    all_errors = errors + identity_errors + binary_errors
    binary_checks = [check for identity in identities.values() for check in identity['binary_file_checks']]
    binary_files_verified = strict_binaries and len(binary_checks) == 4 and all(check['status'] == 'verified' for check in binary_checks)
    recorded_identity_verified = set(identities) == {'before', 'after'} and not identity_errors and all(identity['recorded_build_identity_verified'] for identity in identities.values())
    report = {'audit_kind': 'Independent standard-library append evidence audit; no producer statistics imported', 'script': str(Path(__file__).resolve()), 'script_sha256': digest(__file__), 'evidence_root': str(root), 'verified': not all_errors, 'verification_scope': 'raw evidence plus recorded identities and actual binary files' if strict_binaries else 'raw evidence and available recorded identities; actual binary files not checked', 'raw_data_verified': raw_data_verified, 'recorded_build_identities_verified': recorded_identity_verified, 'binary_files_verified': binary_files_verified, 'binary_verification_mode': args.binary_verification, 'errors': all_errors, 'raw_data_errors': errors, 'recorded_identity_errors': identity_errors, 'binary_file_errors': binary_errors, 'expected_products': 48, 'products_found': len(products), 'accepted_products': sum(p['accepted'] for p in products), 'manifests': manifests, 'binary_identities': identities, 'products': products, 'paired_records': pairs, 'summaries': summaries, 'reference_audit_comparison': reference, 'scope_notes': ['Warm/cold remain separate. Cold means store drop/reopen before each append, not OS page-cache eviction.', 'append_ns brackets append_canonical only. Cold operation CPU/write brackets exclude reopen but include process-sample overhead. append_phase counters include reopen and sampling; both are reported separately.', '100-Hz counters mean 10ms ticks; per-append cold brackets are coarse and sums of separately rounded brackets can amplify quantization. Zero is preserved, not interpreted as CPU-free work.', 'Full lifecycle includes initialization/history seeding/append/reopen verification/cleanup. Absolute through-cleanup counters and start-to-end deltas are separately named.', 'max_sampled memory uses lifecycle endpoints to match prior reports. max_all_observed memory additionally includes every cold operation endpoint. Both are sampled maxima, not actual peaks.', 'Missing/invalid counters are retained with reasons, partial memory maxima stay partial, and ratios with zero denominators remain undefined. Summaries retain pair positions and explicitly report available/required counts.', 'Outliers, zero values and adverse observations are never trimmed. Medians are medians of matched pair ratios, never ratios of unpaired medians.', 'Manifest paths are retained historical identity data. Actual binary files are verified only in strict mode; archive mode can recompute raw data without original host files and explicitly reports that actual binaries were not checked. No current HEAD is consulted.', 'Product accepted/oracle/test-log fields verify retained assertions; this script cannot independently recover the deleted original databases or prove runtime isolation. Per-product records do not contain argv, so executable binding relies on the retained cohort manifest.', 'No performance attribution is assigned to individual source changes. Three pairs do not establish parity or population nonregression.']}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, allow_nan=False) + '\n')
    print(json.dumps({'verified': report['verified'], 'accepted_products': report['accepted_products'], 'products': len(products), 'raw_data_verified': raw_data_verified, 'recorded_build_identities_verified': recorded_identity_verified, 'binary_files_verified': binary_files_verified, 'binary_verification_mode': args.binary_verification, 'errors': all_errors, 'output': str(args.output), 'script_sha256': report['script_sha256']}))
    return 0 if report['verified'] else 1

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path('/home/capi/.cache/rustx-pr485-next/final-cohorts'))
    parser.add_argument('--output', type=Path, default=Path('/home/capi/.cache/rustx-pr485-next/final-append-audit.json'))
    parser.add_argument('--binary-verification', choices=('strict', 'off'), default='strict', help='strict checks actual binary files (default); off recomputes archived evidence without original host binaries')
    parser.add_argument('--before-identity', type=Path)
    parser.add_argument('--after-identity', type=Path)
    parser.add_argument('--expect-before-sha256')
    parser.add_argument('--expect-after-sha256')
    parser.add_argument('--reference-audit', type=Path)
    raise SystemExit(audit(parser.parse_args()))
