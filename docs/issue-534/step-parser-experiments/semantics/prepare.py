from pathlib import Path
root=Path('/tmp/pr556-step-semantics');source=Path('/tmp/pr556-occt-build/occt/src')
s=(source/'StepFile/StepFile_Read.cxx').read_text();s='#include "StepSemantic.hxx"\n'+s
s=s.replace('void StepFile_Interrupt(', 'class DiagnosticScanner : public step::scanner { public: DiagnosticScanner(StepFile_ReadData* data,std::istream* input):step::scanner(data,input){} ~DiagnosticScanner(){StepSemantic::ScannerFlags(yyin.bad(),yyin.fail(),yyin.eof());} };\n\nvoid StepFile_Interrupt(')
s=s.replace('step::scanner aScanner','DiagnosticScanner aScanner')
s=s.replace('    undirec->SetRecord (nr, ident, typrec, nbarg);','    StepSemantic::Record(nr, ident, typrec, nbarg);\n    undirec->SetRecord (nr, ident, typrec, nbarg);')
s=s.replace('        undirec->AddStepParam (nr, val, typa);','        StepSemantic::Argument((int)typa, val);\n        undirec->AddStepParam (nr, val, typa);')
s=s.replace('    undirec->InitParams(nr);','    StepSemantic::EndRecord();\n    undirec->InitParams(nr);')
(root/'src/StepFile_Read.cxx').write_text(s)
s=(source/'Interface/Interface_FileReaderTool.cxx').read_text();s='#include "StepSemantic.hxx"\n'+s
s=s.replace('anent = LoadedEntity(num);','anent = LoadedEntity(num);\n        StepSemantic::Entity(num, anent.IsNull() ? "<null>" : anent->DynamicType()->Name());')
(root/'src/Interface_FileReaderTool.cxx').write_text(s)
