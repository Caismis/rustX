#!/usr/bin/env python3
"""Count completed strace calls, joining interleaved unfinished/resumed records."""
import argparse
from collections import Counter
import gzip
import hashlib
import json
from pathlib import Path
import re


def summarize(path):
    pending = {}
    counts, errors, syncs, writes = Counter(), Counter(), Counter(), Counter()
    unresolved = 0
    raw_hash = hashlib.sha256()
    opener = gzip.open if path.suffix == '.gz' else open
    with opener(path, 'rb') as stream:
        for raw in stream:
            raw_hash.update(raw)
            matched = re.match(r'^(\d+)\s+(.*)', raw.decode('utf-8', errors='replace'))
            if not matched:
                continue
            pid, call = matched.groups()
            if '<unfinished ...>' in call:
                if pid in pending:
                    unresolved += 1
                pending[pid] = call.split('<unfinished ...>')[0]
                continue
            resumed = re.match(r'<\.\.\. \w+ resumed>(.*)', call)
            if resumed:
                beginning = pending.pop(pid, None)
                if beginning is None:
                    unresolved += 1
                    continue
                call = beginning + resumed[1]
            function = re.match(r'(write|pwrite64|fsync|fdatasync|fcntl|unlink|rename)\(', call)
            if not function:
                continue
            result = call.rsplit(' = ', 1)
            if len(result) != 2:
                unresolved += 1
                continue
            result = re.match(r'(-?\d+)(?:\s+(\w+))?', result[1])
            if not result:
                unresolved += 1
                continue
            name, returned = function[1], int(result[1])
            counts[name] += 1
            if returned < 0:
                errors[f'{name}:{result[2]}'] += 1
                continue
            descriptor = re.search(r'^\w+\(\d+<([^>]+)>', call)
            filename = descriptor[1] if descriptor else ''
            category = ('wal' if filename.endswith('-wal') else
                        'shm' if filename.endswith('-shm') else
                        'journal' if filename.endswith('-journal') else
                        'database' if filename.endswith('.sqlite') else
                        'sqlite_temp' if Path(filename).name.startswith('etilqs_') else 'other')
            if name in ('write', 'pwrite64'):
                writes[category] += returned
            if name in ('fsync', 'fdatasync'):
                syncs[category] += 1
    return {'raw_sha256': raw_hash.hexdigest(), 'completed_calls': counts,
            'errors': errors, 'successful_syncs_by_path': syncs,
            'successful_write_bytes_by_path': writes,
            'unresolved_calls': unresolved + len(pending),
            'scope': 'strace -f: runtime/driver plus provider and helper child processes; database categories isolate named SQLite files',
            'limitations': 'OS return-byte accounting, not physical media traffic. Other includes files, pipes and sockets. Not owner-level SQL/transaction counts.'}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('trace', type=Path)
    args = parser.parse_args()
    print(json.dumps(summarize(args.trace), indent=2))
