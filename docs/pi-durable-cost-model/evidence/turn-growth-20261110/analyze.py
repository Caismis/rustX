#!/usr/bin/env python3
"""Recompute the report's SQL, stack and sampled-page write attribution."""
import collections
import gzip
import importlib.util
import json
import lzma
from pathlib import Path
import re
import sqlite3

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parents[3]
spec = importlib.util.spec_from_file_location('growth', REPO / 'benchmarks/runtime/turn-growth.py')
growth = importlib.util.module_from_spec(spec)
spec.loader.exec_module(growth)


def save(name, value):
    (ROOT / name).write_text(json.dumps(value, indent=2) + '\n')


def read_json(path):
    return json.loads(path.read_text() if path.exists() else gzip.open(str(path) + '.gz', 'rt').read())


def group(name):
    if name.startswith(('request_snapshots', 'sqlite_autoindex_request_snapshots')):
        return 'request snapshots'
    if name == 'events':
        return 'event rows'
    if name.startswith('events_') or name.startswith('sqlite_autoindex_events'):
        return 'event indexes'
    if name.startswith(('request_trace_catalog', 'request_trace_definition', 'sqlite_autoindex_request_trace')):
        return 'Trace catalog'
    if name.startswith(('native_input_', 'request_source_', 'sqlite_autoindex_request_source')):
        return 'input manifest and Source'
    if name.startswith(('content_', 'sqlite_autoindex_content')):
        return 'content frontiers/layout'
    if name.startswith(('message_ledger', 'transcript_', 'surface_', 'observation_headers',
                        'observation_block_', 'sqlite_autoindex_message_ledger',
                        'sqlite_autoindex_observation_headers', 'sqlite_autoindex_observation_block',
                        'sqlite_autoindex_transcript', 'sqlite_autoindex_surface')):
        return 'canonical/transcript/layout'
    return 'other'


def sql_analysis():
    results = {}
    for kind in ['text', 'tool']:
        path = ROOT / 'baseline' / kind / 'cut-512.sqlite'
        db = sqlite3.connect(f'file:{path}?mode=ro', uri=True)
        snapshots = []
        for i, (raw,) in enumerate(db.execute('SELECT snapshot_json FROM request_snapshots ORDER BY started_sequence')):
            data = json.loads(raw)
            associations = data['tool_associations']
            snapshots.append({'request_index': i, 'turn': i // (2 if kind == 'tool' else 1) + 1,
                'metadata_bytes': len(raw.encode()), 'associations': len(associations),
                'association_bytes': len(json.dumps(associations, ensure_ascii=False, separators=(',', ':')).encode())})
        windows = []
        wire = json.loads((ROOT / 'baseline' / kind / 'provider.json').read_text())['requests']
        for start in range(1, 513, 32):
            subset = [s for s in snapshots if start <= s['turn'] < start + 32]
            request_count = 2 if kind == 'tool' else 1
            wire_subset = wire[(start - 1) * request_count:(start + 31) * request_count]
            windows.append({'turn_midpoint': start + 15.5,
                'metadata_bytes_per_turn': sum(s['metadata_bytes'] for s in subset) / 32,
                'association_bytes_per_turn': sum(s['association_bytes'] for s in subset) / 32,
                'wire_body_bytes_per_turn': sum(s['body_bytes'] for s in wire_subset) / 32})
        owners = [{'name': name, 'pages': n, 'bytes': size, 'payload': payload, 'group': group(name)}
            for name, n, size, payload in db.execute('SELECT name,count(*),sum(pgsize),sum(payload) FROM dbstat GROUP BY name ORDER BY sum(pgsize) DESC')]
        groups = collections.Counter()
        for owner in owners:
            groups[owner['group']] += owner['bytes']
        result = json.loads((ROOT / 'baseline' / kind / 'result.json').read_text())
        increments = []
        prior = result['windows'][0]
        for w in result['windows'][1:]:
            n = prior['dbstat'][0]['rows']
            m = w['dbstat'][0]['rows']
            increments.append({'turn_end': w['turn_end'],
                **{k: m.get(k, 0) - n.get(k, 0) for k in [
                    'native_input_nodes', 'request_source_nodes', 'events',
                    'request_trace_catalogs', 'request_trace_definitions']}})
            prior = w
        results[kind] = {'schema': db.execute('SELECT schema_version FROM rustx_store').fetchone()[0],
            'requests': len(snapshots), 'metadata_bytes': sum(s['metadata_bytes'] for s in snapshots),
            'association_bytes': sum(s['association_bytes'] for s in snapshots),
            'first_request': snapshots[0], 'last_request': snapshots[-1],
            'owners': owners, 'retained_groups': groups, 'windows': windows,
            'logical_slopes': growth.regress(windows, ['metadata_bytes_per_turn', 'association_bytes_per_turn', 'wire_body_bytes_per_turn']),
            'row_increments': increments}
        db.close()
    save('sql-attribution.json', results)


def cpu_category(stack):
    # Exclusive ownership by the innermost recognizable rustX subsystem.
    # Inline functions may be absent in release symbols. Keep unclassified work.
    rules = [
        ('model::occurrence', 'tool association/correlation'),
        ('model::adapter::correlation', 'tool association/correlation'),
        ('model::adapter', 'provider adapter'),
        ('model::invocation::validate_content_modalities', 'provider adapter'),
        ('sqlite::native_request', 'native request metadata'),
        ('sqlite::request::snapshot', 'native request metadata'),
        ('sqlite::observation_statistics', 'statistics'),
        ('sqlite::manifest', 'input manifest and Source'),
        ('sqlite::observation_request::composed', 'input manifest and Source'),
        ('sqlite::observation_request', 'input manifest and Source'),
        ('sqlite::canonical', 'canonical commit/read'),
        ('sqlite::journal', 'event commit'),
        ('sqlite::event_insert', 'event commit'),
        ('sqlite::request', 'request commit'),
        ('sqlite::observation', 'observation/read projection'),
        ('rustx::runtime_client', 'observation/read projection'),
        ('rustx::observation', 'observation/read projection'),
        ('rustx::context', 'context'),
        ('model::selected_input', 'selected input'),
        ('model::snapshot', 'native request metadata'),
    ]
    for frame in reversed(stack.split(';')):
        for pattern, category in rules:
            if pattern in frame:
                return category
    return 'other/unclassified'


def cpu_analysis():
    results = {}
    for kind in ['text', 'tool']:
        cells = {}
        for phase in ['low128', 'high128']:
            path = ROOT / f'cpu-{phase}-{kind}.folded'
            if not path.exists() and not Path(str(path) + '.gz').exists():
                return
            counts = collections.Counter()
            named = collections.Counter()
            sqlite_samples = 0
            folded = path.read_text() if path.exists() else gzip.open(str(path) + '.gz', 'rt').read()
            for line in folded.splitlines():
                stack, raw = line.rsplit(' ', 1)
                count = int(raw)
                counts[cpu_category(stack)] += count
                sqlite_samples += count * (';sqlite3' in stack)
                for pattern in ['sqlite::observation::messages<', 'model::occurrence::freeze_associations',
                                'model::adapter::validation::prepare_request', 'sqlite::native_request']:
                    if pattern in stack:
                        named[pattern] += count
            cells[phase] = {'samples': sum(counts.values()), 'counts': counts,
                            'sqlite_inclusive_samples': sqlite_samples, 'named_inclusive': named}
        differences = {key: (cells['high128']['counts'].get(key, 0) - cells['low128']['counts'].get(key, 0)) * 2 / 128
            for key in cells['high128']['counts'].keys() | cells['low128']['counts'].keys()}
        results[kind] = {'cells': cells, 'high_minus_low_ms_per_turn_at_500hz': differences,
            'total_difference_ms_per_turn': sum(differences.values()),
            'method': 'Exclusive innermost recognizable rustX owner, full symbol stacks. 128 turns per cell. '
                      '2 ms/sample at 500 Hz. Diagnostic only, no inline debug expansion; negative deltas retained.'}
    save('cpu-attribution.json', results)


def trace_analysis():
    results = {}
    for kind in ['text', 'tool']:
        directory = ROOT / 'strace' / kind
        if not (directory / 'result.json').exists() and not (directory / 'result.json.gz').exists():
            return
        data = read_json(directory / 'result.json')
        initial = json.loads((directory / 'session.json').read_text())['initial_dbstat'][0]['page_owners']
        cells = []
        for w in data['windows']:
            cells.append({'turn_start': w['turn_start'], 'turn_end': w['turn_end'],
                'turn_midpoint': w['turn_midpoint'], 'start': w['before']['epoch_ns'] / 1e9,
                'end': w['after']['epoch_ns'] / 1e9, 'groups': collections.Counter(),
                'owners': collections.Counter(), 'syscalls': collections.Counter(),
                'page_owners': w['dbstat'][0]['page_owners'], 'prior': initial})
            initial = w['dbstat'][0]['page_owners']
        pending, wal_pages = {}, {}
        path = directory / 'server-1.strace'
        if path.exists():
            stream = path.open()
        elif Path(str(path) + '.gz').exists():
            stream = gzip.open(str(path) + '.gz', 'rt')
        else:
            stream = lzma.open(str(path) + '.xz', 'rt')
        orphaned = 0
        for raw in stream:
            m = re.match(r'^(\d+)\s+([0-9.]+)\s+(.*)', raw)
            if not m:
                continue
            pid, stamp, line = m.groups()
            if '<unfinished ...>' in line:
                pending[pid] = (stamp, line.split('<unfinished ...>')[0])
                continue
            if line.startswith('<...'):
                if pid not in pending:
                    orphaned += 1
                    continue
                stamp, prefix = pending.pop(pid)
                line = prefix + re.sub(r'^<\.\.\. \w+ resumed>', '', line)
            cell = next((c for c in cells if c['start'] <= float(stamp) <= c['end']), None)
            result = re.search(r' = (-?\d+)', line)
            if not result or int(result[1]) < 0:
                continue
            call = line.split('(', 1)[0]
            if cell:
                cell['syscalls'][call] += 1
            m = re.match(r'pwrite64\(\d+<([^>]+)>, "([^"]*)"(?:\.\.\.)?, (\d+), (\d+)\)', line)
            if not m:
                continue
            encoded_path, encoded, size, offset = m.groups()
            filename = bytes.fromhex(encoded_path.replace('\\x', '')).decode()
            size, offset, actual = int(size), int(offset), int(result[1])
            if filename.endswith('conversation.sqlite-wal'):
                payload = bytes.fromhex(encoded.replace('\\x', ''))
                if size == 24 and offset >= 32:
                    wal_pages[(filename, offset + 24)] = int.from_bytes(payload[:4], 'big')
                    if cell:
                        cell['groups']['WAL framing'] += actual
                    continue
                page = wal_pages.get((filename, offset))
                channel = 'wal'
            elif filename.endswith('conversation.sqlite'):
                page = offset // 2048 + 1 if size == 2048 else None
                channel = 'main'
            else:
                if cell:
                    cell['groups']['other files'] += actual
                continue
            if cell:
                owner = cell['page_owners'].get(str(page), 'unmapped')
                previous = cell['prior'].get(str(page))
                if previous is not None and previous != owner:
                    owner = 'owner-changed'
                cell['owners'][channel + '/' + owner] += actual
                cell['groups'][group(owner) if owner not in ['unmapped', 'owner-changed'] else owner] += actual
        stream.close()
        for cell in cells:
            del cell['page_owners'], cell['prior']
            for key, value in cell['groups'].items():
                cell[key + '_bytes_per_turn'] = value / (cell['turn_end'] - cell['turn_start'] + 1)
        keys = sorted({key for cell in cells for key in cell if key.endswith('_bytes_per_turn')})
        for cell in cells:
            for key in keys:
                cell.setdefault(key, 0)
        results[kind] = {'windows': cells, 'slopes': growth.regress(cells, keys),
            'unmatched_resumptions': orphaned + len(pending),
            'method': 'Actual successful pwrite64 payloads, WAL frame page numbers. Page owners from end-of-window dbstat; '
                      'pages with a different start owner are explicitly owner-changed. Not authenticated WAL replay; '
                      'within-window page reuse may remain. No physical SSD-byte or causal-isolation claim.'}
    save('write-attribution.json', results)


if __name__ == '__main__':
    sql_analysis()
    cpu_analysis()
    trace_analysis()
