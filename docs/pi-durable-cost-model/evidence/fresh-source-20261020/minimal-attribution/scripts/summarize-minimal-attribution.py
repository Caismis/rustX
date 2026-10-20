#!/usr/bin/env python3
"""Rebuild diagnostic summaries from preserved raw products; never launches products."""
import collections
import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import re
import struct

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--root',type=Path,default=Path(__file__).resolve().parent / 'evidence/minimal-attribution-20261020')
ROOT = parser.parse_args().root.resolve()
HZ = os.sysconf('SC_CLK_TCK')

def load(path):
    return json.loads(path.read_text())

def save(name, data):
    (ROOT/name).write_text(json.dumps(data, indent=2)+'\n')

def proc(value):
    stat = value['stat'].rsplit(')', 1)[1].split()
    io = dict((k, int(v)) for k, v in (line.split(':') for line in value['io'].splitlines()))
    smaps = {k:int(v.split()[0]) for k,v in (line.split(':') for line in value.get('smaps_rollup','').splitlines() if ':' in line)}
    return {'cpu_s':(int(stat[11])+int(stat[12]))/HZ, 'starttime':int(stat[19]),
            'minor_faults':int(stat[7]), 'major_faults':int(stat[9]), **io, **smaps}

def profile(path):
    raw=path.read_bytes(); header=struct.unpack_from('<5Q',raw); assert header[:3]==(0,3,0)
    offset=40; stacks=[]
    while True:
        count,depth=struct.unpack_from('<QQ',raw,offset); offset+=16
        stack=struct.unpack_from('<'+'Q'*depth,raw,offset); offset+=depth*8
        if count==0 and depth==1 and stack[0]==0: break
        stacks.append((count,stack))
    mappings=[]
    for line in raw[offset:].decode().splitlines():
        m=re.match(r'([0-9a-f]+)-([0-9a-f]+) (\S+) ([0-9a-f]+) [0-9a-f:]+ (\d+)\s*(/.*)',line)
        if m: mappings.append((int(m[1],16),int(m[2],16),m[6]))
    modules=collections.Counter()
    for count,stack in stacks:
        modules[next((name for lo,hi,name in mappings if lo<=stack[0]<hi),'no-file-map')]+=count
    return {'samples':sum(c for c,_ in stacks), 'period_us':header[3], 'stack_records':len(stacks),
            'max_recorded_stack_depth':max((len(s) for _,s in stacks),default=0),
            'bytes':len(raw),'sha256':hashlib.sha256(raw).hexdigest(),'leaf_modules':dict(modules)}

orchestration=load(ROOT/'orchestration.json')
runs=[]; cpu_rows=[]; phase_rows=[]; symbol_reports={}; profile_reports={}
for run in orchestration['runs']:
    name=run['label']+'-'+run['mode']; folder=ROOT/name
    raw=load(folder/'sample.json'); outer=load(folder/'sample.process.json')
    assert raw['oracle']=='passed' and outer['accepted'] and outer['product_exit']==outer['provider_exit']==0
    assert len(raw['samples'])==96 and len([s for s in raw['samples'] if s['measured']])==32
    assert raw['model_time']=='2026-10-19 00:00:00'
    life={i['phase']:i for i in raw['lifecycle']}; observations=[proc(i['process']) for i in raw['lifecycle']+raw['samples']]
    steady=[s for s in raw['samples'] if s['measured']]; before=proc(raw['samples'][63]['process']); after=proc(steady[-1]['process'])
    result={'run':name,'source':run['binary_identity']['head'],'binary_sha256':run['binary_identity']['sha256'],
            'start_utc':run['start_utc'],'end_utc':run['end_utc'],'oracle':'passed',
            'steady_mean_settled_ms':sum(s['settled_us'] for s in steady)/32/1000,
            'steady_mean_first_consumed_ms':sum(s['first_consumed_publication_us'] for s in steady)/32/1000,
            'steady_sampled_cpu_per_turn_ms':(after['cpu_s']-before['cpu_s'])/32*1000,
            'steady_native_cpu_s':after['cpu_s']-before['cpu_s'],
            'steady_notification_records':steady[-1]['notification_records']-raw['samples'][63]['notification_records'],
            'steady_wchar':after['wchar']-before['wchar'],'steady_write_bytes':after['write_bytes']-before['write_bytes'],
            'steady_cancelled_write_bytes':after['cancelled_write_bytes']-before['cancelled_write_bytes'],
            'sampled_max_pss_kib':max(i['Pss'] for i in observations),'sampled_max_pss_anon_kib':max(i['Pss_Anon'] for i in observations),
            'complete_controller_lifecycle_s':outer['complete_lifecycle_ns']/1e9,
            'through_product_exit_s':outer['through_product_exit_ns']/1e9,
            'reopen_ms':raw['reopen_us']/1000,
            'controller_self_cpu_s':run['controller_self_cpu_seconds']}
    runs.append(result)
    for record in run['wait4']:
        role='native' if run['binary_identity']['binary'] in record['argv'] else 'provider-command-including-waited-descendants'
        cpu_rows.append({'run':name,'role':role,'pid':record['pid'],'user_s':record['user_seconds'],
                         'system_s':record['system_seconds'],'total_s':record['user_seconds']+record['system_seconds'],
                         'wait4_status':record['wait4_status'],'maxrss_kib':record['maxrss_kib']})
    assert sorted(row['role'] for row in cpu_rows if row['run']==name)==['native','provider-command-including-waited-descendants']
    for start,end,label in [('after_initialization','after_turns','all-96-turns'),('after_turns','after_late_snapshot','late-snapshot'),
                            ('after_late_snapshot','after_verification','request-verification'),('after_final_writer_close','after_reopen_verification','reopen-verification'),
                            ('after_reopen_verification','after_cleanup','cleanup')]:
        a,b=life[start],life[end]; pa,pb=proc(a['process']),proc(b['process']);assert pa['starttime']==pb['starttime']
        phase_rows.append({'run':name,'phase':label,'wall_s':(b['elapsed_us']-a['elapsed_us'])/1e6,'native_cpu_s':pb['cpu_s']-pa['cpu_s'],
                           'wchar':pb['wchar']-pa['wchar'],'write_bytes':pb['write_bytes']-pa['write_bytes'],'read_bytes':pb['read_bytes']-pa['read_bytes']})
    if run['mode']=='cpu':
        profiles={p.name:profile(p) for p in sorted(folder.glob('*.cpu'))};profile_reports[name]=profiles
        text=(folder/'before_measured_turns-cumulative.txt').read_text();total=int(re.search(r'Total: (\d+) samples',text)[1]);rows=[]
        for line in text.splitlines():
            m=re.match(r'\s*(\d+)\s+([\d.]+)%\s+([\d.]+)%\s+(\d+)\s+([\d.]+)%\s+(.+)',line)
            if m:rows.append({'flat':int(m[1]),'inclusive':int(m[4]),'symbol':m[6]})
        assert sum(i['flat'] for i in rows)==total==profiles['before_measured_turns.cpu']['samples']
        unknown=sum(i['flat'] for i in rows if re.fullmatch(r'0x[\da-f]+',i['symbol']))
        unknown_root=max(i['inclusive'] for i in rows if re.fullmatch(r'0x[\da-f]+',i['symbol']))
        symbol_reports[name]={'total_samples':total,'unresolved_leaf_samples':unknown,'unresolved_leaf_fraction':unknown/total,
                              'largest_unresolved_frame_inclusive_samples':unknown_root,
                              'scope':'Flat leaf proportion uses all weighted samples. Inclusive unknown root frames can occur in otherwise resolved Rust stacks; this is not the fraction of unusable leaves. O3/debug0 does not establish inline source ownership.',
                              'top_flat':sorted(rows,key=lambda x:x['flat'],reverse=True)[:25],
                              'top_inclusive':sorted(rows,key=lambda x:x['inclusive'],reverse=True)[:60], 'all_symbols':rows,
                              'nominal_sample_seconds':total*.004,'nearby_steady_proc_cpu_s':result['steady_native_cpu_s'],
                              'nominal_sample_seconds_over_nearby_cpu':total*.004/result['steady_native_cpu_s'],
                              'actual_sampling_loss_rate':None,'loss_scope':'Not exposed by this signal sampler. Nominal time/nearby proc CPU is NOT a loss-rate estimate. Standard signal coalescing, multithread timer semantics, boundary and proc accounting differ. PROFILE evictions are flushed hash entries, not dropped samples.'}

save('run-summary.json',runs);save('process-cpu.json',cpu_rows);save('phase-summary.json',phase_rows)
save('profile-integrity.json',profile_reports);save('steady-symbolization.json',symbol_reports)
for name,rows in [('run-summary.csv',runs),('process-cpu.csv',cpu_rows),('phase-summary.csv',phase_rows)]:
    with (ROOT/name).open('w') as stream:
        w=csv.DictWriter(stream,fieldnames=list(rows[0]));w.writeheader();w.writerows(rows)
print(json.dumps({'runs':runs,'process_cpu':cpu_rows,'unresolved':{k:{n:v[n] for n in ['total_samples','unresolved_leaf_samples','unresolved_leaf_fraction','nominal_sample_seconds_over_nearby_cpu']} for k,v in symbol_reports.items()}},indent=2))
