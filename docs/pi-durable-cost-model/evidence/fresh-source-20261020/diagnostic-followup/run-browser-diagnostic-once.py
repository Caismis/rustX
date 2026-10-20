#!/usr/bin/env python3
"""One reviewed geometry replay; external watchdog/accounting, unchanged fixture."""
import ast
import datetime
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import time

os.umask(0o077)
root=Path('/workspace/pr485-study');repo=root/'repo';evidence=root/'evidence'
review=root/'diagnostic-fixture-review';fixture=review/'benchmarks/pi-durable-inspector-fixture/v3'
commit='cef4d78ad234b8cf60acb563a5e42bc0263ea0bf'
image='mcr.microsoft.com/playwright:v1.63.0-noble@sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7'
def now():return datetime.datetime.now(datetime.timezone.utc).isoformat()
driver=root/'run-browser-cohort.py'
tree=ast.parse(driver.read_text())
functions=[node for node in tree.body if isinstance(node,ast.FunctionDef) and node.name in ['sha','space','assert_idle','restore']]
assert len(functions)==4
exec(compile(ast.Module(body=functions,type_ignores=[]),str(driver),'exec'),globals())
run_id='geometry-short-large-cef4d78a-'+datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')
parent=root/'private-diagnostics';parent.mkdir(mode=0o700,exist_ok=True)
output=parent/run_id;output.mkdir(mode=0o700)
manifest={'run_id':run_id,'scope':'Single diagnostic replay only; no performance acceptance or causal Native inference',
          'fixture_commit':commit,'source_head':'89d6b618eeba8afaf4b95af4406057330baa60a6','case':'short-large',
          'review_authorization':'Coordinator reported independent read-only review with no verified blocker to bounded diagnostic replay',
          'global_watchdog_seconds':600,'disk_guard_bytes_strict':5368709120,'private_output':str(output),
          'start_utc':now(),'outer_driver_sha256':sha(__file__),'reused_preflight_driver_sha256':sha(driver),
          'raw_output_sensitivity':'Private pending inspection: console/close reasons may contain arbitrary content',
          'observation_limits':['Metadata extraction precedes diagnostic timestamps and adds overhead',
            '50000-event buffers retain beginning; inspect dropped counts for missing terminal events',
            'Cleanup may prevent diagnostics.json; missing exit record is not absence of exit',
            'Browser receive callback is not paint; original wire repair barrier remains limited']}
path=output/'orchestration.json'
statuspath=root/'STATUS.json'
def save():
 path.write_text(json.dumps(manifest,indent=2)+'\n')
 state=json.loads(statuspath.read_text());state['updated_utc']=now()
 state['active_browser_diagnostic']={'run_id':run_id,'manifest':str(path),'fixture_commit':commit,
   'state':manifest.get('state','preflight'),'updated_utc':state['updated_utc'],'product_pid':manifest.get('fixture_pid')}
 state['status']='Reviewed single-case geometry browser diagnostic replay: '+manifest.get('state','preflight')
 statuspath.write_text(json.dumps(state,indent=2)+'\n')
save()
assert subprocess.check_output(['git','rev-parse','HEAD'],cwd=review,text=True).strip()==commit
assert not subprocess.check_output(['git','status','--porcelain','--',str(fixture.relative_to(review))],cwd=review,text=True).strip()
names=subprocess.check_output(['git','ls-tree','-r','--name-only',commit,'--',str(fixture.relative_to(review))],cwd=review,text=True).splitlines()
expected={Path(name).name:hashlib.sha256(subprocess.check_output(['git','show',commit+':'+name],cwd=review)).hexdigest() for name in names}
actual={p.name:sha(p) for p in fixture.iterdir() if p.is_file()};assert actual==expected
old=evidence/'products/browser-baseline-vs-geometry-v2/1-short-large-geometry/failure.json'
manifest['original_failure']={'path':str(old),'sha256':sha(old),'bytes':old.stat().st_size}
frontend=json.loads((evidence/'common-frontend.json').read_text())
for relative,digest in frontend['dist_sha256'].items():assert sha(repo/'web-console/dist'/relative)==digest
for relative,digest in frontend['native_node_modules_sha256'].items():assert sha(repo/'web-console'/relative)==digest
manifest.update(fixture_sha256=expected,frontend_identity_sha256=sha(evidence/'common-frontend.json'),idle=assert_idle(),space_before=space())
for entry in Path('/proc').iterdir():
 if entry.name.isdigit():
  try:
   comm=(entry/'comm').read_text().strip();state=(entry/'stat').read_text().rsplit(')',1)[1].split()[0]
   assert state=='Z' or comm not in ['google-pprof','addr2line'],(entry.name,comm,state)
  except OSError:pass
manifest['image_inspect']=json.loads(subprocess.check_output(['docker','image','inspect',image],text=True))
identity,host,copied=restore('geometry');manifest.update(native_build=identity,host_build=host,helpers_restored=copied)
assert identity['head']==manifest['source_head']
identitypath=repo/'source-identity.json';prior_identity=identitypath.read_bytes() if identitypath.exists() else None
identitypath.write_text(json.dumps({'head':identity['head'],'proposal':'diagnostic-only fixture '+commit})+'\n')
binary=evidence/'build-geometry/host/inspector-host'
argv=['node','--experimental-transform-types',str(fixture/'run.mjs'),str(repo),str(binary),'short-large',str(output/'product')]
env=os.environ|{'TMPDIR':str(root/'tmp'),'TZ':'UTC'};assert not env.get('LD_PRELOAD')
manifest.update(argv=argv,cwd=str(repo),runtime_overrides={'TMPDIR':env['TMPDIR'],'TZ':'UTC','LD_PRELOAD':None},
                space_at_product_start=space(),product_start_utc=now(),product_start_monotonic_ns=time.monotonic_ns(),state='running')
save();print(json.dumps({'run_id':run_id,'manifest':str(path),'argv':argv}),flush=True)
started=time.monotonic();minimum_free=shutil.disk_usage(root).free;usage=None;process=None
def descendants(pid):
 try:children=[int(x) for x in (Path('/proc')/str(pid)/'task'/str(pid)/'children').read_text().split()]
 except OSError:return []
 return children+[p for child in children for p in descendants(child)]
try:
 with (output/'fixture.stdout').open('w') as stdout,(output/'fixture.stderr').open('w') as stderr,(output/'external-processes.jsonl').open('w') as counters:
  process=subprocess.Popen(argv,cwd=repo,env=env,stdout=stdout,stderr=stderr,start_new_session=True)
  manifest['fixture_pid']=process.pid;save();signal_sent=None;last_save=time.monotonic()
  while True:
   pid,status,usage=os.wait4(process.pid,os.WNOHANG)
   if pid:
    process.returncode=os.waitstatus_to_exitcode(status);break
   records=[]
   for pid in [process.pid,*descendants(process.pid)]:
    p=Path('/proc')/str(pid)
    try:records.append({'pid':pid,'comm':(p/'comm').read_text().strip(),'stat':(p/'stat').read_text(),'io':(p/'io').read_text()})
    except OSError:pass
   free=shutil.disk_usage(root).free;minimum_free=min(minimum_free,free)
   counters.write(json.dumps({'utc':now(),'monotonic_ns':time.monotonic_ns(),'free_bytes':free,'processes':records})+'\n');counters.flush()
   elapsed=time.monotonic()-started
   if signal_sent is None and (free<=5368709120 or elapsed>=600):
    manifest['watchdog_trigger']={'utc':now(),'elapsed_s':elapsed,'free_bytes':free,'reason':'disk-guard' if free<=5368709120 else '600s-global-bound'}
    os.killpg(process.pid,signal.SIGTERM);signal_sent=time.monotonic();save()
   elif signal_sent is not None and time.monotonic()-signal_sent>=10:
    os.killpg(process.pid,signal.SIGKILL);manifest['watchdog_kill_utc']=now();signal_sent=float('inf');save()
   if time.monotonic()-last_save>=5:manifest['elapsed_s']=elapsed;manifest['minimum_free_bytes']=minimum_free;save();last_save=time.monotonic()
   time.sleep(1)
 manifest.update(state='product-exited',fixture_exit=process.returncode,product_end_utc=now(),product_end_monotonic_ns=time.monotonic_ns(),minimum_free_bytes=minimum_free,
   fixture_wait4={'user_seconds':usage.ru_utime,'system_seconds':usage.ru_stime,'maxrss_kib':usage.ru_maxrss,'minor_faults':usage.ru_minflt,'major_faults':usage.ru_majflt,
    'scope':'Direct fixture Node plus waited descendant rusage may be included; do not sum again with descendant CPU.'})
 save()
 # This read-only query occurs after the product and retains only owned-container events.
 try:
  data=subprocess.check_output(['docker','events','--since',manifest['product_start_utc'],'--until',manifest['product_end_utc'],'--filter','type=container','--format','{{json .}}'],text=True,timeout=10)
  events=[json.loads(line) for line in data.splitlines() if line.strip()]
  owned=[row for row in events if row.get('Actor',{}).get('Attributes',{}).get('name','').startswith('pr485-inspector-'+str(process.pid)+'-')]
  (output/'docker-events.json').write_text(json.dumps(owned,indent=2)+'\n');manifest['owned_docker_events']=len(owned)
 except Exception as error:manifest['docker_events_unavailable']=str(error)
finally:
 if prior_identity is None:identitypath.unlink(missing_ok=True)
 else:identitypath.write_bytes(prior_identity)
 manifest.update(space_after=space(check=False),original_failure_unchanged=sha(old)==manifest['original_failure']['sha256'],
  native_binary_unchanged=sha(binary)==host['binary_sha256'],fixture_unchanged={p.name:sha(p) for p in fixture.iterdir() if p.is_file()}==expected)
 manifest['diagnostics_present']=(output/'product/diagnostics.json').exists();manifest['sample_present']=(output/'product/sample.json').exists();manifest['failure_present']=(output/'product/failure.json').exists()
 manifest['state']='finished';save();print(json.dumps({'run_id':run_id,'fixture_exit':manifest.get('fixture_exit'),'diagnostics_present':manifest['diagnostics_present'],'sample_present':manifest['sample_present'],'failure_present':manifest['failure_present'],'minimum_free_bytes':minimum_free}),flush=True)
