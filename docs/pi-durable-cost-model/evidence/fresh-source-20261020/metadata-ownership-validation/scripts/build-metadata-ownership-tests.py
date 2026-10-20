#!/usr/bin/env python3
"""Guarded isolated lib-test compilation; no cleanup, profiling or product timing."""
from pathlib import Path
import datetime,hashlib,json,os,shutil,signal,subprocess,time
root=Path('/workspace/pr485-study');repo=root/'metadata-ownership-candidate';out=root/'evidence/metadata-ownership-candidate-20261020'
out.mkdir(exist_ok=False)
def now():return datetime.datetime.now(datetime.timezone.utc).isoformat()
def sha(p):return hashlib.file_digest(Path(p).open('rb'),'sha256').hexdigest()
def point():
 s=os.statvfs(root)
 return {'utc':now(),'monotonic_ns':time.monotonic_ns(),'available_bytes':s.f_bavail*s.f_frsize,
  'memory_current':Path('/sys/fs/cgroup/memory.current').read_text().strip()}
assert subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip()=='623b41b869a95c888eb04f907cbf607232699a2f'
assert point()['available_bytes']>7*2**30
protected={}
for label in ['baseline','geometry','scoped']:
 b=root/'evidence'/('build-'+label);identity=json.loads((b/'identity.json').read_text());host=json.loads((b/'host/identity.json').read_text())
 for item in json.loads((b/'preserved-artifacts.json').read_text())['artifacts']:protected[item['preserved']]=item['sha256']
 for name in ['conformance','rustx','bash-supervisor','interactive-supervisor']:
  p=b/name;protected[str(p)]=sha(p)
 protected[str(b/'host/inspector-host')]=host['binary_sha256']
 assert sha(b/'conformance')==identity['sha256']
for path,digest in protected.items():assert sha(path)==digest,path
old=root/'evidence/products/browser-baseline-vs-geometry-v2/1-short-large-geometry/failure.json';protected[str(old)]=sha(old)
(out/'protected-identities-before.json').write_text(json.dumps(protected,indent=2)+'\n')
source=['src/runtime_client/projection.rs','src/runtime_client/projection/metadata_ownership_tests.rs','Cargo.toml','Cargo.lock','rust-toolchain.toml']
env=os.environ.copy()
for key in ['CARGO_ENCODED_RUSTFLAGS','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER']:env.pop(key,None)
overrides={'PATH':'/workspace/.cargo/bin:'+env['PATH'],'CARGO_HOME':'/workspace/.cargo','RUSTUP_HOME':'/workspace/.rustup','RUSTUP_TOOLCHAIN':'1.98.1',
 'CARGO_TARGET_DIR':str(root/'target'),'CARGO_PROFILE_RELEASE_OPT_LEVEL':'3','CARGO_PROFILE_RELEASE_DEBUG':'0','CARGO_INCREMENTAL':'0',
 'RUSTFLAGS':'-Copt-level=3 -Cdebuginfo=0','BINDGEN_EXTRA_CLANG_ARGS':'-isystem /usr/lib/gcc/x86_64-linux-gnu/14/include',
 'LIBCLANG_PATH':'/usr/lib/llvm-19/lib','TMPDIR':str(root/'tmp'),'CARGO_BUILD_JOBS':'1'}
env.update(overrides)
assert not env.get('LD_PRELOAD')
argv=['cargo','test','--release','--locked','--offline','--lib','--no-run','--message-format=json','-vv','-j','1']
record={'start_utc':now(),'base':'623b41b869a95c888eb04f907cbf607232699a2f','branch':subprocess.check_output(['git','branch','--show-current'],cwd=repo,text=True).strip(),
 'argv':argv,'cwd':str(repo),'environment_overrides':overrides,'source_sha256':{name:sha(repo/name) for name in source},'before':point(),
 'guard_floor_bytes':5*2**30,'soft_stop_available_bytes':6*2**30,'no_cleanup':True,'purpose':'Compile equivalence/regression tests only; no performance product run',
 'outer_script_sha256':sha(__file__)}
path=out/'build-orchestration.json'
def save():
 path.write_text(json.dumps(record,indent=2)+'\n')
 p=root/'STATUS.json';state=json.loads(p.read_text());state['updated_utc']=now();state['status']='Metadata ownership-only candidate on baseline623b41b: '+record.get('state','preflight')
 state['active_metadata_candidate']={'manifest':str(path),'branch':record['branch'],'state':record.get('state','preflight'),'pid':record.get('pid')};p.write_text(json.dumps(state,indent=2)+'\n')
save()
for p in Path('/proc').iterdir():
 if p.name.isdigit():
  try:
   comm=(p/'comm').read_text().strip();state=(p/'stat').read_text().rsplit(')',1)[1].split()[0]
   assert state=='Z' or comm not in ['cargo','rustc','rust-lld','cc1','cc1plus','conformance','inspector-host','google-pprof','addr2line','perf','xz','gzip'],(p.name,comm,state)
  except OSError:pass
record.update(state='compiling',launch=point());save();print(json.dumps({'state':'launch','manifest':str(path),'available_bytes':record['launch']['available_bytes'],'argv':argv}),flush=True)
with (out/'cargo.stdout.jsonl').open('w') as stdout,(out/'cargo.stderr').open('w') as stderr,(out/'build-resources.jsonl').open('w') as log:
 process=subprocess.Popen(argv,cwd=repo,env=env,stdout=stdout,stderr=stderr,start_new_session=True);record['pid']=process.pid;save();started=time.monotonic();minimum=point()['available_bytes'];stopped=None
 while process.poll() is None:
  p=point();minimum=min(minimum,p['available_bytes']);log.write(json.dumps(p)+'\n');log.flush()
  if stopped is None and (p['available_bytes']<=6*2**30 or time.monotonic()-started>2700):
   record['guard_stop']=p|{'reason':'soft-space-stop' if p['available_bytes']<=6*2**30 else '45-minute-build-bound'};os.killpg(process.pid,signal.SIGTERM);stopped=time.monotonic();save()
  elif stopped is not None and time.monotonic()-stopped>10:os.killpg(process.pid,signal.SIGKILL);stopped=float('inf')
  time.sleep(.5)
 record.update(exit=process.returncode,end_utc=now(),after=point(),minimum_observed_available_bytes=minimum,state='compile-finished')
 for name in source:assert sha(repo/name)==record['source_sha256'][name]
 save()
print(json.dumps({'exit':record['exit'],'minimum_available_bytes':minimum,'guard_stop':record.get('guard_stop')}),flush=True)
