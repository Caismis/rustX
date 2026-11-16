#!/usr/bin/env python3
import json, hashlib, pathlib, re
ROOT=pathlib.Path(__file__).resolve().parent;RAW=ROOT/'raw'
def load(p):return json.loads(p.read_text())
def diff(a,b,path=''):
 out=[]
 if isinstance(a,dict) and isinstance(b,dict):
  assert a.keys()==b.keys(),path
  for k in a:out+=diff(a[k],b[k],path+'/'+k)
 elif isinstance(a,list) and isinstance(b,list):
  if len(a)!=len(b):out.append({'path':path,'original':a,'recomputed':b})
  else:
   for i,(x,y) in enumerate(zip(a,b)):out+=diff(x,y,path+f'/{i}')
 elif a!=b:out.append({'path':path,'original':a,'recomputed':b})
 return out
datasets=[('direct','direct-cohorts/recomputed.json','direct.stdout'),('confirmation','confirmation-cohorts/recomputed.json','confirmation.stdout'),('pi','final-cohorts/pi-recomputed.json','pi.stdout'),('append','final-cohorts/append-audit.json','append-recomputed.json'),('three-engine','final-cohorts/three-engine-audit.json','three-engine-recomputed.json')]
summary={}
for name,old,new in datasets:
 a=load(RAW/old);b=load(ROOT/new);changes=diff(a,b)
 for d in changes:
  p=d['path'];v=d['recomputed']
  if name=='pi':
   allowed={'/identity_audit/actual_binary_checks':[], '/identity_audit/actual_binary_files_verified':False,
    '/identity_audit/mode':'raw-only','/identity_audit/strict_identity_verified':False,'/strict_identity_verified':False,
    '/identity_audit/identity_file':str(RAW/'candidate-f-build/identity.json')}
   assert p in allowed and v==allowed[p],d
  elif name=='append':
   if p.endswith('/path') or p in ['/script','/evidence_root']:
    assert v==d['original'].replace('/home/capi/.cache/rustx-pr485-cost-reduction',str(RAW)),d
   elif p.endswith('/identity_path'):
    expected='baseline-d-build-identity.json' if '/before/' in p else 'candidate-f-build/identity.json'
    assert v==str(RAW/expected),d
   elif re.fullmatch('/binary_identities/(before|after)/binary_file_checks/[01]/(status|actual_sha256)',p):
    assert v==('not_attempted' if p.endswith('/status') else None),d
   else:
    expected={'/binary_files_verified':False,'/binary_verification_mode':'off',
     '/verification_scope':'raw evidence and available recorded identities; actual binary files not checked'}
    assert p in expected and v==expected[p],d
  elif name=='three-engine':
   allowed={'/identity/external_check_omission_reason':'raw-only: no external binary, Git checkout, or dependency files accessed',
    '/identity/measurement_commit_verified':False,'/identity/external_actual_source_checked':False,'/identity/current_harness_head':None,
    '/identity/native_files_hashed':0,'/identity/current_harness_file_bytes_match':None,'/identity/mode':'raw-only',
    '/identity/external_actual_binary_checked':False,'/identity/strict_identity_verified':False,'/identity/harness_files_hashed':0,
    '/identity/measurement_commit_files_hashed':0,'/identity/current_head_differs_from_measurement':None,'/mode':'raw-only',
    '/strict_identity_verified':False,'/acceptance_scope':'archived evidence and arithmetic only; external identity unchecked',
    '/dataset':str(RAW/'final-cohorts/three-engine')}
   assert p in allowed and v==allowed[p],d
  else:assert False,d
 assert a.get('verified',a.get('accepted')) and b.get('verified',b.get('accepted'))
 summary[name]={'accepted':True,'exit':0,'all_numeric_values_and_recorded_identity_content_equal':True,
  'products':len(b.get('products',b.get('rows',[]))),'allowed_differences':changes}
report=load(ROOT/'archive-verification.json');assert all(s['exit']==0 for s in report['steps']) and len(report['steps'])==5
report.update(accepted=True,replay_comparisons=summary,total_accepted_formal_products=sum(x['products'] for x in summary.values()),
 comparison_script_sha256=hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest(),
 conclusion='All five replays exit zero. Every numeric/product/group value and recorded identity content matches; only listed relocation and explicitly omitted external identity checks differ. No original external binary/source/dependency verification is claimed by this replay.')
assert report['total_accepted_formal_products']==284
(ROOT/'archive-verification.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps({'accepted':True,'products':284,'payload_files':report['payload_files'],'payload_bytes':report['payload_bytes'],'differences':{k:len(v['allowed_differences']) for k,v in summary.items()}}))
