from pathlib import Path
import json,shlex,subprocess,os,hashlib
root=Path('/tmp/pr556-step-semantics');base=Path('/tmp/pr556-build-O3/build');commands=json.loads((base/'compile_commands.json').read_text());evidence={'compile':[],'variants':[]};env={**os.environ,'BINARYEN_CORES':'2','EMCC_CORES':'2'};replacements={}
for name in ['StepFile_Read.cxx','Interface_FileReaderTool.cxx','StepSemantic.cxx']:
 item=next(c for c in commands if c['file'].endswith('/'+('StepFile_Read.cxx' if name=='StepSemantic.cxx' else name)));args=shlex.split(item['command']);old=args[args.index('-o')+1];obj=str(root/'objects'/(name+'.o'));args[args.index('-o')+1]=obj;args[args.index('-c')+1]=str(root/'src'/name);args.insert(1,'-I'+str(root/'src'));subprocess.run(args,cwd=item['directory'],env=env,check=True);evidence['compile'].append({'name':name,'args':args,'sha256':hashlib.sha256((root/'src'/name).read_bytes()).hexdigest()});
 if name!='StepSemantic.cxx':replacements[old]=obj
 print(name,'compiled',flush=True)
line=next(l for l in (base/'build.ninja').read_text().splitlines() if l.startswith('build Release/occt-import-js.js:'));objects=line.split(': ',1)[1].split()[1:];assert len(objects)==4775
line=next(l for l in Path('/tmp/pr556-build-O3/effective-commands.txt').read_text().splitlines()if' -o Release/occt-import-js.js 'in l and' -c 'not in l);args=shlex.split(line);start=next(i for i,a in enumerate(args)if a.endswith('/em++'));args=args[start:args.index('&&',start)];tailkey=next(o for o in objects if o.endswith('/StepFile_ReadData.cxx.o'));lexkey=next(o for o in objects if o.endswith('/lex.step.cxx.o'))
for name,tail,lex in [('original',False,False),('tail',True,False),('lexer',False,True),('combined',True,True)]:
 target=root/name;target.mkdir(exist_ok=True);replaced=dict(replacements)
 if tail:replaced[tailkey]='/tmp/pr556-step-tail-only/objects/StepFile_ReadData.cxx.o'
 if lex:replaced[lexkey]='/tmp/pr556-step-lexer-only/objects/lex.step.cxx.o'
 inputs=[replaced.get(o,str(base/o))for o in objects]+[str(root/'objects/StepSemantic.cxx.o')];rsp=target/'link.rsp';rsp.write_text('\n'.join(inputs)+'\n');argv=[('@'+str(rsp))if a.startswith('@')else a for a in args];argv[argv.index('-o')+1]=str(target/'occt-import-js.js');subprocess.run(argv,cwd=base,env=env,check=True);evidence['variants'].append({'name':name,'link':argv,'replacements':replaced});print(name,'linked',flush=True)
(root/'build-evidence.json').write_text(json.dumps(evidence,indent=2)+'\n')
