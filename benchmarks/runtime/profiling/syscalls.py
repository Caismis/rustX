#!/usr/bin/env python3
"""Attach a controller-owned tracer to its product child before provider readiness."""
import argparse,json,os,subprocess,sys
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from controller import run,TEST
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True)
a=p.parse_args();root=a.output.resolve();root.mkdir(parents=True,exist_ok=False)
original=subprocess.Popen;tracers=[]
def start(argv,*args,**kwargs):
 product=original(argv,*args,**kwargs)
 if argv[0]==str(a.binary.resolve()):
  tracer=original(['strace','-f','-yy','-s','80','-e','trace=openat,close,pread64,pwrite64,write,fsync,fdatasync,unlink,unlinkat','-o',str(root/'calls.strace'),'-p',str(product.pid)],stderr=subprocess.PIPE,text=True)
  tracers.append(tracer)
  line=tracer.stderr.readline()
  (root/'attach.json').write_text(json.dumps({'product_pid':product.pid,'tracer_pid':tracer.pid,'attach':line,'scope':'Attached before controller releases provider readiness; dynamic loader/startup before attach is excluded. Product, provider and tracer are controller-owned siblings. Profile outside formal timings.'}))
  assert 'attached' in line,line
 return product
subprocess.Popen=start
env={k:v for k,v in os.environ.items() if not any(s in k.upper() for s in ('TOKEN','SECRET','CREDENTIAL','API_KEY'))};env.update(RUSTX_BENCH_CASE='short-small',RUSTX_BENCH_OUTPUT=str(root/'sample.json'))
r=run([str(a.binary.resolve()),TEST,'--exact','--ignored','--nocapture'],root/'sample','short-small',env,Path('test-support/fake-provider').resolve())
for tracer in tracers:
 stderr=tracer.communicate()[1];(root/'tracer.log').write_text(stderr);assert tracer.returncode==0
print(json.dumps({'accepted':r['accepted'],'error':r.get('error')}));assert r['accepted']
