from pathlib import Path
import shutil,json,hashlib
root=Path('/tmp/pr556-step-semantics/cases')
for p in Path('/tmp/pr556-step-lexer-only/cases').glob('*.step'):shutil.copyfile(p,root/p.name)
base=(root/'nested-lists.step').read_text();header=base.split('DATA;')[0]+'DATA;\n';end='\nENDSEC;\nEND-ISO-10303-21;'
cases={
'empty-nested-typed.step':"#1 = TEST_ENTITY((),((),(1,2),(3,(4,5))),LENGTH_MEASURE(1.25),TEST_TYPE((1,2)),*,$,.T.,.UNSPECIFIED.,#2,'one''two',-3,1.23E-4);\n#2 = CARTESIAN_POINT('point',(1.,2.,3.));",
'complex-typed.step':"#1 = (LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));\n#2 = MEASURE_REPRESENTATION_ITEM('length',LENGTH_MEASURE(1.25),#1);",
'error-recovery.step':"#1 = CARTESIAN_POINT('before',(1.,2.,3.));\n#2 = TEST_ENTITY((1,,3));\n#3 = CARTESIAN_POINT('after',(4.,5.,6.));",
'long-string-comment.step':"/*"+' comment '*1300+"*/\n#1=TEST_ENTITY('"+'a'*8180+"''quoted\n"+'b'*4000+"',('tail'),LENGTH_MEASURE(2.5));\n#2=CARTESIAN_POINT('after',(4.,5.,6.));",
'large-list-order.step':"#1=TEST_ENTITY(("+','.join(str(i) for i in range(10000))+"));\n#2=CARTESIAN_POINT('after',(4.,5.,6.));"
}
for name,data in cases.items():(root/name).write_text(header+data+end)
shutil.copyfile('/tmp/rustx-pr556/web-console/test/fixtures/geometry/cube.step',root/'cube.step')
manifest=[{'name':p.name,'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()}for p in sorted(root.glob('*.step'))];(root.parent/'cases-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
