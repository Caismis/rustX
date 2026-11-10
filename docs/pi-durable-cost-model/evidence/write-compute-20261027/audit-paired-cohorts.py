#!/usr/bin/env python3
"""Independent raw arithmetic for saved D/F direct and confirmation cohorts only."""
import collections
import hashlib
import json
import math
from pathlib import Path
import statistics
import subprocess

ROOT = Path(__file__).resolve().parent
REPO = Path('/home/capi/.codex/worktrees/pr485-pi-performance/rustX')
REV = '2800be25a650305d5daf9f543ffbf7d8f62ea79b'
HASHES = {'before':'8ec26a4a08bde085f46ee40159d3870f7ead675bec39f42cb4f3e55df1d1d233', 'after':'7ca19d99e1a241e5e33627819fe837a81788d26609072d8c41a1a0ab805ef109'}
PHASES = ['before_initialization','after_provider_start','after_initialization','after_turns','after_late_snapshot','after_verification','after_shutdown','after_driver_drop','after_final_writer_close','after_reopen_verification','after_cleanup']
POLICY = ['mean_turn_ms','turn_cpu_ms_per_turn','through_cleanup_self_cpu_ms','through_cleanup_process_write_mib','max_sampled_anon_pss_mib']

def load(path):
    return json.loads(path.read_text())

def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream,'sha256').hexdigest()

def same(a,b):
    if isinstance(a,float) or isinstance(b,float):
        return a is not None and b is not None and math.isclose(a,b,rel_tol=1e-12,abs_tol=1e-12)
    return a == b

def counters(point):
    raw = point['process']
    assert set(raw) == {'stat','io','smaps_rollup'}
    cpu = None
    identity = None
    if raw['stat'] is not None:
        tail = raw['stat'].rsplit(')',1)[1].split()
        user,system = int(tail[11]),int(tail[12])
        assert min(user,system)>=0
        cpu = user+system
        identity = (int(raw['stat'].split('(',1)[0]),int(tail[19]))
    io = dict(line.split(':',1) for line in (raw['io'] or '').splitlines())
    memory = dict(line.split(':',1) for line in (raw['smaps_rollup'] or '').splitlines() if ':' in line)
    result = {'cpu':cpu,'write':None if 'write_bytes' not in io else int(io['write_bytes']),'identity':identity}
    for key in ['Rss','Pss','Pss_Anon']:
        if key in memory:
            value,unit = memory[key].split()
            assert unit=='kB' and int(value)>=0
            result[key]=int(value)
        else:
            result[key]=None
    return result

def derive(sample,proc,case,mode):
    history={'short-small':8,'short-large':64}.get(case,0)
    turns=32 if history else 1
    requests={'short-small':40,'short-large':96,'tool':2}.get(case,1)
    assert proc['accepted'] and proc['product_exit']==proc['provider_exit']==0
    assert proc['case']==case and proc['observer_mode']==mode
    provider=proc['provider_report']
    assert provider['ok'] and provider['complete'] and not provider['failures']
    assert provider['requestCount']==requests and provider['stepsMatched']==provider['stepsSettled']==provider['stepsTotal']==requests
    assert provider['stepStates']==['settled']*requests
    assert sample['oracle']=='passed' and sample['workload_version']==3 and sample['measurement_version']==4
    assert sample['case']==case and sample['observer_mode']==mode and sample['model_time']=='2026-10-19 00:00:00'
    assert sample['history']==history and sample['measured']==turns
    assert sample['wire_requests']==provider['requests'] and len(sample['wire_requests'])==requests
    assert sample['live_requests']==sample['reopened_requests'] and len(sample['live_requests'])==requests
    assert sample['verified_live_requests']==sample['verified_reopened_requests']==requests
    points=sample['samples']; life=sample['lifecycle']
    assert len(points)==history+turns and [p['turn'] for p in points]==list(range(history+turns))
    assert [p['measured'] for p in points]==[i>=history for i in range(history+turns)]
    assert all(p['observer_mode']==mode and type(p['settled_us']) is int and p['settled_us']>=0 for p in points)
    assert [p['phase'] for p in life]==PHASES
    assert [p['elapsed_us'] for p in life]==sorted(p['elapsed_us'] for p in life)
    sampled=[counters(p) for p in points]; lifecycle=[counters(p) for p in life]
    assert len({p['identity'] for p in sampled+lifecycle if p['identity'] is not None})==1
    for sequence in [sampled,lifecycle]:
        for key in ['cpu','write']:
            values=[p[key] for p in sequence if p[key] is not None]
            assert values==sorted(values)
    start=sampled[history-1] if history else lifecycle[2]
    end=sampled[-1]; final=lifecycle[-1]
    cpu=None if start['cpu'] is None or end['cpu'] is None else (end['cpu']-start['cpu'])*10/turns
    assert cpu is None or cpu>=0
    result={'mean_turn_ms':sum(p['settled_us'] for p in points[history:])/turns/1000,
        'turn_cpu_ms_per_turn':cpu,
        'through_cleanup_self_cpu_ms':None if final['cpu'] is None else final['cpu']*10,
        'through_cleanup_process_write_mib':None if final['write'] is None else final['write']/1048576,
        'lifecycle_ms':(life[-1]['elapsed_us']-life[0]['elapsed_us'])/1000}
    null_reasons={}
    for key,name in [('Rss','rss'),('Pss','pss'),('Pss_Anon','anon_pss')]:
        values=[p[key] for p in sampled+lifecycle]
        metric=f'max_sampled_{name}_mib'
        result[metric]=None if any(v is None for v in values) else max(values)/1024
        if result[metric] is None:null_reasons[metric]=f'{sum(v is None for v in values)} missing {key} snapshots'
    if result['turn_cpu_ms_per_turn'] is None:null_reasons['turn_cpu_ms_per_turn']='missing boundary stat'
    if result['through_cleanup_self_cpu_ms'] is None:null_reasons['through_cleanup_self_cpu_ms']='missing cleanup stat'
    if result['through_cleanup_process_write_mib'] is None:null_reasons['through_cleanup_process_write_mib']='missing cleanup write_bytes'
    return result,null_reasons

report={'verified':True,'source_revision':REV,'identity':{},'datasets':{},'scope':'Independent raw arithmetic and admission records; existing native semantic oracles checked, not fully reimplemented. No products/tests/builds run.'}
for engine,path in [('before',Path('/home/capi/.cache/rustx-pr485-next/candidate-d-build/conformance')),('after',ROOT/'candidate-f-build/conformance')]:
    identity=load(path.parent/'identity.json')
    assert identity['verified'] and identity['build_exit']==0 and identity['sha256']==HASHES[engine]==digest(path)
    actual_tree=subprocess.check_output(['git','rev-parse',identity['head']+':src'],cwd=REPO,text=True).strip()
    assert actual_tree==identity['production_tree']
    for name,sha in identity['harness'].items(): assert digest(REPO/name)==sha
    report['identity'][engine]=identity
assert report['identity']['after']['head']==REV
assert report['identity']['after']['production_tree']=='a86302ed48b415de91cac782bd156dc935b8e517'
assert report['identity']['before']['rustc']==report['identity']['after']['rustc']
assert report['identity']['before']['harness']==report['identity']['after']['harness']

last_end=None
for name,pairs,cases,critical in [('direct-cohorts',3,['short-small','short-large','stream','large','tool','cancel'],2.919985580355516),('confirmation-cohorts',7,['stream','cancel'],1.9431802805153022)]:
    base=ROOT/name
    commands=load(base/'commands.json')
    assert all(s['status']=='complete' and s['exit']==0 for s in commands['steps'])
    assert all(a['end_utc']<=b['start_utc'] for a,b in zip(commands['steps'],commands['steps'][1:]))
    if last_end:assert last_end<=commands['steps'][0]['start_utc']
    last_end=commands['steps'][-1]['end_utc']
    old=load(base/'recomputed.json')
    expected_count=2*len(cases)*pairs*2
    assert old['verified'] and old['accepted_products']==expected_count
    old_products={p['file']:p for p in old['products']}
    old_pairs={(p['cohort'],p['case'],p['pair'],p['metric']):p for p in old['paired_records']}
    old_groups={(p['cohort'],p['case'],p['metric']):p for p in old['summaries']}
    products=[]; all_pairs=[]; groups=[]; request_comparisons=0; comparisons=0
    for cohort,mode in [('direct-no-live','no-live-subscriber'),('direct-live','live-subscriber')]:
        directory=base/cohort
        manifest=load(directory/'manifest.json')
        assert manifest['head']==REV and manifest['pairs']==pairs and manifest['cases']==cases and manifest['clock_ticks']==100 and manifest['pair_offset']==0
        assert manifest['observer_mode']==mode
        for engine in ['before','after']:
            assert manifest['executables'][engine]['sha256']==HASHES[engine]
            assert manifest['executables'][engine]['path']==report['identity'][engine]['binary']
        expected={f'{pair}-{case}-{engine}.process.json' for pair in range(pairs) for case in cases for engine in ['before','after']}
        assert {p.name for p in directory.glob('*.process.json')}==expected
        assert not list(directory.glob('*-comparison-error.log'))
        logs=[json.loads(line) for line in (base/(cohort+'.stdout')).read_text().splitlines() if line.startswith('{')]
        expected_order=[(case,engine) for pair in range(pairs) for case in cases for engine in (['before','after'] if pair%2==0 else ['after','before'])]
        assert len(logs)==len(expected_order)
        for row,(case,engine) in zip(logs,expected_order):
            assert row['case']==case and row['argv'][0]==manifest['executables'][engine]['path'] and row['accepted']
        by_group=collections.defaultdict(list)
        for case in cases:
            for pair in range(pairs):
                metrics={}
                for engine in ['before','after']:
                    stem=f'{pair}-{case}-{engine}'; file=f'{cohort}/{stem}.json'
                    sample=load(directory/(stem+'.json')); proc=load(directory/(stem+'.process.json'))
                    assert proc['argv']==[manifest['executables'][engine]['path'],'agent_loop::runtime_benchmark::measure_runtime','--exact','--ignored','--nocapture']
                    values,reasons=derive(sample,proc,case,mode)
                    prior=old_products[file]
                    assert prior['raw_sha256']==digest(directory/(stem+'.json')) and prior['process_sha256']==digest(directory/(stem+'.process.json'))
                    assert set(values)==set(prior['metrics']) and all(same(v,prior['metrics'][k]) for k,v in values.items())
                    metrics[engine]=values
                    products.append({'file':file,'metrics':values,'null_reasons':reasons,'raw_sha256':prior['raw_sha256'],'process_sha256':prior['process_sha256']})
                comparison=load(directory/f'{pair}-{case}-comparison.json')
                assert comparison['case']==case and comparison['observer_mode']==mode and comparison['workload_version']==3 and comparison['model_time']=='2026-10-19 00:00:00'
                checked=comparison['checked']; expected_requests={'short-small':40,'short-large':96,'tool':2}.get(case,1)
                assert len(checked)==expected_requests and [p['index'] for p in checked]==list(range(expected_requests))
                assert all(p['coreSemanticEqual'] and p['fullSemanticRequestEqual'] and p['rustToolsSha256']==p['piToolsSha256'] and p['rustToolNames']==p['piToolNames'] for p in checked)
                comparisons+=1;request_comparisons+=expected_requests
                for metric,a in metrics['after'].items():
                    b=metrics['before'][metric]
                    ratio=None if a is None or b is None or a<=0 or b<=0 else a/b
                    prior=old_pairs[(cohort,case,pair,metric)]
                    assert same(prior['numerator'],a) and same(prior['denominator'],b) and same(prior['ratio'],ratio)
                    row={'cohort':cohort,'case':case,'pair':pair,'metric':metric,'before':b,'after':a,'difference':None if a is None or b is None else a-b,'ratio':ratio}
                    all_pairs.append(row);by_group[(case,metric)].append(row)
        for (case,metric),rows in by_group.items():
            ratios=[r['ratio'] for r in rows]; prior=old_groups[(cohort,case,metric)]
            assert prior['values']==ratios and prior['n']==pairs
            derived={'cohort':cohort,'case':case,'metric':metric,'n':pairs,'pairs':rows}
            if any(v is None for v in ratios):
                assert prior['state']=='indeterminate_missing_or_nonpositive'
                derived.update(state=prior['state'],null_reason='At least one missing or nonpositive paired metric')
            else:
                logs=[math.log(v) for v in ratios]; center=statistics.mean(logs); width=critical*statistics.stdev(logs)/math.sqrt(pairs)
                checks={'median':statistics.median(ratios),'minimum':min(ratios),'maximum':max(ratios),'geometric_mean':math.exp(center),'lower90':math.exp(center-width),'upper90_one_sided95':math.exp(center+width)}
                assert prior['state']=='determinate' and all(same(prior[k],v) for k,v in checks.items())
                before=[r['before'] for r in rows];after=[r['after'] for r in rows];diffs=[r['difference'] for r in rows]
                derived.update(state='determinate',ratio_summary=checks,before_median=statistics.median(before),before_range=[min(before),max(before)],after_median=statistics.median(after),after_range=[min(after),max(after)],difference_median=statistics.median(diffs),difference_range=[min(diffs),max(diffs)],after_lower=sum(v<0 for v in diffs),equal=sum(v==0 for v in diffs),after_higher=sum(v>0 for v in diffs))
            groups.append(derived)
    assert len(products)==expected_count and comparisons==len(old['comparisons']) and request_comparisons==old['request_comparisons']
    report['datasets'][name]={'accepted_products':len(products),'comparisons':comparisons,'request_comparisons':request_comparisons,'null_metric_count':sum(len(p['null_reasons']) for p in products),'all_producer_arithmetic_equal':True,'recorded_execution_order_verified':True,'products':products,'groups':groups}

report['method_limits']=['100Hz self CPU has 10ms ticks; complete CPU includes startup/driver/verification/close, while measured-turn CPU uses the explicit prior sample or after_initialization.', 'Memory is maximum of sampled Pss_Anon, not continuous peak; process write_bytes is not device write traffic.', 'Confirmation cases were selected after initial adverse observations. Report both cohorts separately; no sample exclusion or pooled median to hide initial results.', 'Pairwise log t intervals are exploratory estimates under independence/shape assumptions, not familywise tests or proof of universal nonregression.']
(ROOT/'paired-cohorts-audit.json').write_text(json.dumps(report,indent=2,allow_nan=False)+'\n')
print(json.dumps({name:{k:v for k,v in data.items() if k not in ['products','groups']} for name,data in report['datasets'].items()},indent=2))
