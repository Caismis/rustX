#!/usr/bin/env python3
"""Second raw arithmetic/record audit; no producer code or products are run.

This checks existing native semantic-oracle records, not all semantic assertions.
It is intentionally separate from the three existing strict auditors.
"""
import argparse
import collections
import hashlib
import json
import math
from pathlib import Path
import statistics

P = argparse.ArgumentParser()
P.add_argument('--root', type=Path, default=Path(__file__).resolve().parent)
P.add_argument('--output', type=Path)
A = P.parse_args()
BASE = A.root / 'final-cohorts'
OUT = A.output or A.root / 'final-products-review.json'
F = '7ca19d99e1a241e5e33627819fe837a81788d26609072d8c41a1a0ab805ef109'
D = '8ec26a4a08bde085f46ee40159d3870f7ead675bec39f42cb4f3e55df1d1d233'
REV = '2800be25a650305d5daf9f543ffbf7d8f62ea79b'
PI = '90d863f7c1183b965f099855715d6e88c7b8a52c'
DS = '687f282e8625c6b8d11ff58211a90524be2e4317'

def load(path):
    return json.loads(path.read_text())

def sha(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()

def close(a, b):
    assert a is None and b is None or a is not None and b is not None and math.isclose(a, b, rel_tol=1e-11, abs_tol=1e-10), (a, b)

def proc(raw, waited=False):
    tail = raw['stat'].rsplit(')', 1)[1].split()
    io = dict(line.split(':', 1) for line in raw['io'].splitlines())
    mem = dict(line.split(':', 1) for line in raw['smaps_rollup'].splitlines() if ':' in line)
    cpu = sum(int(tail[i]) for i in ([11,12,13,14] if waited else [11,12])) * 10
    row = {'cpu': cpu, 'write': int(io['write_bytes']), 'pid': (int(raw['stat'].split('(', 1)[0]), int(tail[19]))}
    for k in ['Rss', 'Pss', 'Pss_Anon']:
        v, unit = mem[k].split()
        assert unit == 'kB' and int(v) >= 0
        row[k] = int(v) / 1024
    assert cpu >= 0 and row['write'] >= 0
    return row

def tree(raw):
    # All current SDK snapshots are one CLI PID. Do not claim arbitrary descendants.
    assert len(raw) == 1
    return proc(next(iter(raw.values())), waited=True)

def monotonic(points):
    assert len({p['pid'] for p in points}) == 1
    for k in ['cpu', 'write']:
        assert [p[k] for p in points] == sorted(p[k] for p in points)

def provider(raw, n):
    assert raw['ok'] and raw['complete'] and not raw['failures'] and not raw['unsettledSteps']
    assert raw['requestCount'] == raw['stepsTotal'] == raw['stepsMatched'] == raw['stepsSettled'] == n
    assert raw['stepStates'] == ['settled'] * n
    assert [r['index'] for r in raw['requests']] == list(range(n))

def runtime(raw, engine, complete):
    h, n = raw['history'], raw['measured']
    samples = raw['samples']; life = raw['lifecycle']
    assert len(samples) == h + n
    assert [s['turn'] for s in samples] == list(range(h + n))
    assert [s['measured'] for s in samples] == [False] * h + [True] * n
    if engine == 'deepseek':
        points = [tree(samples[h-1]['after'] if h else samples[0]['before'])] + [tree(s['after']) for s in samples[h:]]
        wall = [s['settled_ns'] / 1e6 for s in samples[h:]]
    else:
        phases = {p['phase']:p for p in life}
        start = samples[h-1] if h else phases['after_initialization']
        points = [proc(s['process']) for s in [start] + samples[h:]]
        wall = [s['settled_us'] / 1000 for s in samples[h:]]
    monotonic(points)
    row = {'mean_turn_ms': statistics.mean(wall), 'turn_cpu_ms_per_turn': (points[-1]['cpu'] - points[0]['cpu'])/n}
    if complete:
        ends = [proc(s['process']) for s in life]; allp = [proc(s['process']) for s in samples] + ends
        monotonic(ends)
        row.update(through_cleanup_self_cpu_ms=ends[-1]['cpu'], through_cleanup_process_write_mib=ends[-1]['write']/2**20,
                   lifecycle_ms=(life[-1]['elapsed_us'] - life[0]['elapsed_us'])/1000)
        for k, name in [('Rss','rss'), ('Pss','pss'), ('Pss_Anon','anon_pss')]:
            row['max_sampled_' + name + '_mib'] = max(p[k] for p in allp)
    else:
        row.update(turn_write_mib_per_turn=(points[-1]['write']-points[0]['write'])/n/2**20,
                   max_sampled_turn_pss_mib=max(p['Pss'] for p in points[1:]),
                   max_sampled_turn_anon_pss_mib=max(p['Pss_Anon'] for p in points[1:]))
    return row

report = {'accepted': False, 'scope': 'Independent second numeric derivation and existing oracle-record consistency, not semantic-oracle reimplementation or binary provenance attestation.',
          'script_sha256': sha(Path(__file__)), 'durability_equivalence': False, 'products': [], 'groups': [], 'counts': {}}
commands = load(BASE/'commands.json')
assert all(x['status'] == 'complete' and x['exit'] == 0 for x in commands['steps'])
assert all(a['end_utc'] <= b['start_utc'] for a,b in zip(commands['steps'], commands['steps'][1:]))
assert commands['identities']['after']['sha256'] == F and commands['identities']['after']['head'] == REV
assert commands['identities']['before']['sha256'] == D
report['recorded_binary_identities'] = {'F': F, 'D': D, 'F_revision': REV}

# Fresh F/Pi: raw metrics and every existing cross-request admission record.
prior = load(BASE/'pi-recomputed.json')
assert prior['verified'] and prior['strict_identity_verified'] and prior['accepted_products'] == 72
pi_values = {}
for row in prior['products']:
    path = BASE / row['file']; raw = load(path); side = load(path.with_suffix('.process.json'))
    engine = 'rust' if path.stem.endswith('-rust') else 'pi'
    assert raw['oracle'] == 'passed' and side['accepted'] and side['product_exit'] == side['provider_exit'] == 0
    count = len(raw['wire_requests']) if engine == 'rust' else len(raw['requests'])
    provider(side['provider_report'], count)
    if engine == 'pi': assert raw['revision'] == PI and len(raw['reopenedRequests']) == count
    else: assert raw['live_requests'] == raw['reopened_requests'] and raw['verified_reopened_requests'] == count
    assert sha(path) == row['raw_sha256'] and sha(path.with_suffix('.process.json')) == row['process_sha256']
    metrics = runtime(raw, engine, complete=True)
    for k,v in metrics.items(): close(v, row['metrics'][k])
    pi_values[row['file']] = metrics
    report['products'].append({'file': row['file'], 'metrics': metrics, 'requests': count})
requests = 0
for row in prior['comparisons']:
    path = BASE/row['cohort']/f"{row['pair']}-{row['case']}-comparison.json"
    raw = load(path)
    assert sha(path) == row['sha256'] and len(raw['checked']) == row['checked_requests']
    assert all(x['coreSemanticEqual'] and not x['fullSemanticRequestEqual'] for x in raw['checked'])
    requests += len(raw['checked'])
for group in prior['summaries']:
    ratios = []
    for i in range(3):
        stem = f"{group['cohort']}/{i}-{group['case']}"
        ratios.append(pi_values[stem+'-rust.json'][group['metric']]/pi_values[stem+'-pi.json'][group['metric']])
    for a,b in zip(ratios, group['values']): close(a,b)
    for k, v in [('median',statistics.median(ratios)), ('minimum',min(ratios)), ('maximum',max(ratios))]: close(group[k],v)
assert requests == 846 and len(prior['comparisons']) == 36
report['counts']['pi'] = {'products':72, 'comparisons':36, 'paired_requests':requests}

# Canonical append: independently derive the report's five metrics + lifecycle wall.
prior = load(BASE/'append-audit.json')
assert prior['verified'] and prior['accepted_products'] == 48 and prior['binary_files_verified']
append = {}
for row in prior['products']:
    file = f"append-{row['mode']}/{row['pair']}-{row['history']}-{row['engine']}.json"
    path = BASE/file; raw = load(path); side = load(path.with_suffix('.process.json'))
    assert raw['oracle'] == 'passed' and raw['schema'] == 76 and raw['measured'] == 32
    assert side['accepted'] and side['exit'] == 0 and len(raw['append_ns']) == 32
    points = [proc(s['process']) for s in raw['lifecycle']]; monotonic(points)
    metrics = {'mean_append_ms': statistics.mean(raw['append_ns'])/1e6,
               'append_phase_cpu_ms': points[2]['cpu']-points[1]['cpu'],
               'through_cleanup_self_cpu_ms': points[-1]['cpu'],
               'through_cleanup_process_write_mib': points[-1]['write']/2**20,
               'max_sampled_anon_pss_mib': max(s['Pss_Anon'] for s in points),
               'lifecycle_ms': (raw['lifecycle'][-1]['elapsed_us']-raw['lifecycle'][0]['elapsed_us'])/1000}
    for k,v in metrics.items(): close(v,row['metrics'][k]['value'])
    for ext, rec in row['source_files'].items(): assert sha(path.with_suffix(ext)) == rec['sha256']
    append[(row['mode'],row['history'],row['pair'],row['engine'])] = metrics
    report['products'].append({'file':file, 'metrics':metrics})
for group in prior['summaries']:
    item = {'cohort':'append', 'mode':group['mode'], 'history':group['history'], 'metrics':{}}
    for k in metrics:
        a = [append[(group['mode'],group['history'],i,'after')][k] for i in range(3)]
        b = [append[(group['mode'],group['history'],i,'before')][k] for i in range(3)]
        r = [x/y for x,y in zip(a,b)]; saved=group['metrics'][k]['after_over_before']
        for x,y in zip(r,saved['values']): close(x,y)
        for name,v in [('median',statistics.median(r)),('minimum',min(r)),('maximum',max(r))]: close(saved[name],v)
        item['metrics'][k] = {'before':b, 'after':a, 'ratio':r, 'differences':[x-y for x,y in zip(a,b)]}
    assert item['metrics']['through_cleanup_process_write_mib']['ratio'] == [1,1,1]
    report['groups'].append(item)
report['counts']['append'] = {'products':48, 'appends':1536, 'pairs':24}

# Native references: five raw metrics, retained native log/resume structures and counts.
prior = load(BASE/'three-engine-audit.json'); manifest=load(BASE/'three-engine/manifest.json')
assert prior['accepted'] and prior['strict_identity_verified'] and prior['valid_comparisons'] == 12
assert manifest['rust']['sha256'] == F and manifest['pi']['revision'] == PI
assert manifest['harness']['revision'] == REV and manifest['deepseek']['revision'] == DS
assert manifest['deepseek_profile'] == 'sdk' and not manifest['durability_equivalence']
derived = collections.defaultdict(list); req=collections.Counter(); sdk_logs=[]
for row in prior['rows']:
    stem=f"{row['repetition']}-{row['case']}-{row['engine']}"
    path=BASE/'three-engine'/(stem+'/result.json' if row['engine']=='deepseek' else stem+'.json')
    raw=load(path); assert sha(path) == row['raw_result_sha256']
    native=row['engine']=='deepseek'; expected={'short-small':40,'short-large':96}.get(row['case'],1)
    if native:
        assert raw['accepted'] and raw['profile']=='sdk'
        assert raw['oracle']['native_resume'] and raw['oracle']['unchanged_persisted_prefix']
        assert not raw['durability']['fsync_success_acknowledged']
        assert raw['oracle']['ordered_requests']==raw['oracle']['persisted_assistant_messages']==expected+1
        assert raw['first_exit']['exit_code']==raw['verification_exit']['exit_code']==0
        provider(raw['provider_report'],expected); provider(raw['verification_provider_report'],1)
        before=raw['persisted_before_reopen']; after=raw['persisted_after_reopen']
        assert after[:len(before)]==before and after[len(before)]['type']=='session/end-seed'
        assert before[1:]==raw['first_notifications'] and after[len(before)+1:]==raw['verification_notifications']
        assert len(raw['requests'])==expected and len(raw['verification_requests'])==1
        logfiles=list((path.parent/'home/sessions').rglob('session.v4.jsonl.zstd')); assert len(logfiles)==1
        assert sha(logfiles[0])==row['evidence']['compressed_log_sha256']
        kinds=collections.Counter(x['type'] for x in before)
        assert kinds['request/header']==kinds['request/context']==1 and kinds['assistant/message']==expected
        sdk_logs.append({'product':stem, 'before_records':len(before), 'after_records':len(after), 'kinds':dict(kinds)})
        req['deepseek_verification']+=1
    else:
        assert raw['oracle']=='passed'
        side=load(path.with_suffix('.process.json')); assert side['accepted'] and side['product_exit']==side['provider_exit']==0
        provider(side['provider_report'],expected)
        if row['engine']=='rust':assert raw['live_requests']==raw['reopened_requests'] and len(raw['wire_requests'])==expected
        else:assert raw['revision']==PI and len(raw['requests'])==len(raw['reopenedRequests'])==expected
    req[row['engine']]+=expected
    metrics=runtime(raw,row['engine'],complete=False)
    assert not row['unavailable_metrics']
    for k,v in metrics.items():close(v,row[k])
    derived[(row['case'],row['engine'])].append(metrics)
    report['products'].append({'file':str(path.relative_to(BASE)), 'metrics':metrics, 'requests':expected})
for group in prior['groups']:
    rows=derived[(group['case'],group['engine'])]; assert len(rows)==3
    g={'cohort':'three-engine','case':group['case'],'engine':group['engine'],'metrics':{}}
    for k in rows[0]:
        values=[r[k] for r in rows]
        close(statistics.median(values),group['medians'][k]); close(min(values),group['ranges'][k][0]); close(max(values),group['ranges'][k][1])
        g['metrics'][k]={'median':statistics.median(values),'minimum':min(values),'maximum':max(values)}
    report['groups'].append(g)
assert req=={'rust':414,'pi':414,'deepseek':414,'deepseek_verification':12}
report['counts']['three_engine']={'products':36,'comparisons':12,'requests':dict(req),'total_provider_requests':sum(req.values())}
report['sdk_logs']=sdk_logs
report['counts']['total_products']=len(report['products']); assert len(report['products'])==156
report['limitations']=['All numeric samples available in this cohort; no missing values were replaced with zero.',
 'SDK idle is not final flush/fsync. No equal-durability ratios or cross-engine lifecycle rankings.',
 'Source CLI includes supported typert source patch; SDK no partial text notifications; 25 tools and native envelope differ.',
 'Binary/source provenance is the separate retained strict audit, not proved by numeric reproduction.',
 'Retained decoded log is checked; Zstd is hashed but not independently decompressed here.',
 'Three pairs are exploratory. CPU ticks are 10 ms; timing outliers are retained without assigning causes.']
report['accepted']=True
OUT.write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps({'accepted':True,'output':str(OUT),'counts':report['counts']}))
