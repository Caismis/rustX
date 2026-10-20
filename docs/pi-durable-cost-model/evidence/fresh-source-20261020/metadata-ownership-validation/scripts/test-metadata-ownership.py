#!/usr/bin/env python3
"""Serial correctness tests on the compiled candidate; never opt-in measurements."""
from pathlib import Path
import datetime,hashlib,json,os,signal,subprocess,time
root=Path('/workspace/pr485-study');repo=root/'metadata-ownership-candidate';out=root/'evidence/metadata-ownership-candidate-20261020'
def now():return datetime.datetime.now(datetime.timezone.utc).isoformat()
def sha(p):return hashlib.file_digest(Path(p).open('rb'),'sha256').hexdigest()
def free():
 s=os.statvfs(root);return s.f_bavail*s.f_frsize
build_path=out/'build-final-validated.json' if (out/'build-final-validated.json').exists() else out/'build-orchestration.json'
build=json.loads(build_path.read_text());assert build['exit']==0 and not build.get('guard_stop')
for p,digest in build['source_sha256'].items():assert sha(repo/p)==digest
rows=[json.loads(line) for line in (out/'cargo.stdout.jsonl').read_text().splitlines() if line.startswith('{')]
artifacts=[r for r in rows if r.get('reason')=='compiler-artifact'];units=[r for r in artifacts if r['target']['name']=='rustx' and r['profile']['test'] and r.get('executable')]
assert len(units)==1;unit=units[0];binary=Path(unit['executable']);assert unit['profile']['opt_level']=='3' and unit['profile']['debuginfo']==0
env=os.environ|{'LD_LIBRARY_PATH':str(root/'target/release/deps')+':/workspace/.rustup/toolchains/1.98.1-x86_64-unknown-linux-gnu/lib','TMPDIR':str(root/'tmp'),'RUST_BACKTRACE':'1'}
assert not env.get('LD_PRELOAD')
listing=subprocess.check_output([str(binary),'--list'],cwd=repo,env=env,text=True);(out/'libtests-list.txt').write_text(listing)
names=[line.removesuffix(': test') for line in listing.splitlines() if line.endswith(': test')]
new='runtime_client::projection::tests::metadata_ownership_tests::'
specs=[('ownership',new,[]),('projection','runtime_client::projection::tests::',[new,'::scale::']),
 ('checkpoint','runtime_client::checkpoint::',['::scale::']),('settlement','runtime_client::host::tests::settlement_tests::',[])]
record={'start_utc':now(),'scope':'Correctness only;no performance or ignored/live/provider/measurement test opt-in','binary':str(binary),'sha256':sha(binary),'cargo_artifact':unit,
 'cached_dependency_artifacts':sum(r['fresh'] for r in artifacts if r['target']['name']!='rustx'),'runs':[],'guard_floor_bytes':5*2**30,'soft_stop_bytes':6*2**30}
path=out/'test-orchestration.json';assert not path.exists()
def save():
 path.write_text(json.dumps(record,indent=2)+'\n')
 p=root/'STATUS.json';state=json.loads(p.read_text());state['updated_utc']=now();state['status']='Metadata ownership-only candidate correctness validation: '+record.get('state','preparing');state['active_metadata_candidate']['state']=record.get('state','preparing');p.write_text(json.dumps(state,indent=2)+'\n')
save()
for label,filter,skips in specs:
 selected=[name for name in names if filter in name and not any(skip in name for skip in skips)]
 assert selected,(label,filter)
 if label=='ownership':assert len(selected)==8
 assert free()>6*2**30
 argv=[str(binary),filter,'--test-threads=1','--nocapture']
 for skip in skips:argv+=['--skip',skip]
 run={'label':label,'argv':argv,'discovered_matching_tests':len(selected),'start_utc':now(),'available_before':free()};record['runs'].append(run);record['state']='testing-'+label;save();print(json.dumps({'label':label,'selected':len(selected),'binary':str(binary)}),flush=True)
 with (out/(label+'.stdout')).open('w') as stdout,(out/(label+'.stderr')).open('w') as stderr:
  process=subprocess.Popen(argv,cwd=repo,env=env,stdout=stdout,stderr=stderr,start_new_session=True);start=time.monotonic();minimum=free();stop=None
  while process.poll() is None:
   available=free();minimum=min(minimum,available)
   if stop is None and (available<=6*2**30 or time.monotonic()-start>900):
    run['stop']={'utc':now(),'available_bytes':available,'reason':'space' if available<=6*2**30 else '15-minute-correctness-bound'};os.killpg(process.pid,signal.SIGTERM);stop=time.monotonic()
   elif stop is not None and time.monotonic()-stop>10:os.killpg(process.pid,signal.SIGKILL);stop=float('inf')
   time.sleep(.5)
  run.update(exit=process.returncode,end_utc=now(),minimum_observed_available_bytes=minimum,available_after=free())
 save();print(json.dumps({'label':label,'exit':run['exit'],'minimum_available_bytes':minimum}),flush=True)
 if run['exit']!=0:
  record.setdefault('failed_runs',[]).append(label)
  record['state']='test-failed-continuing-independent-regressions';save()
record.update(end_utc=now(),state='tests-failed' if record.get('failed_runs') else 'tests-passed');save()
assert sha(binary)==record['sha256']
print(json.dumps({'state':record['state'],'runs':len(record['runs']),'failed_runs':record.get('failed_runs',[])}),flush=True)
