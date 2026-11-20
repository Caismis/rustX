from pathlib import Path
source=Path('/tmp/pr556-occt-build/occt/src');out=Path('/tmp/pr556-step-analysis/src')
def change(s,old,new,n=1):
 assert s.count(old)>=n,(old,s.count(old));return s.replace(old,new,n)
def span(phase,code):return '{ StepDiagnostic::Span _diag(StepDiagnostic::'+phase+'); '+code+' }'
for relative in ['StepFile/StepFile_Read.cxx','StepData/StepData_StepReaderTool.cxx','IFSelect/IFSelect_WorkSession.cxx','Interface/Interface_FileReaderTool.cxx']:
 s=(source/relative).read_text();s='#include "StepDiagnostic.hxx"\n'+s
 if relative.startswith('StepFile/'):
  s=change(s,'  // if stream is not provided, open file stream here','  StepDiagnostic::Span _total(StepDiagnostic::FileRead);\n  // if stream is not provided, open file stream here')
  s=change(s,'    aLetat = aParser.parse();',span('ScannerParser','aLetat = aParser.parse();'))
  s=change(s,'  Handle(StepData_StepReaderData) undirec =\n    new StepData_StepReaderData(nbhead,nbrec,nbpar, theStepModel->SourceCodePage());', '  StepDiagnostic::Counts(nbhead,nbrec,nbpar);\n  Handle(StepData_StepReaderData) undirec;\n  '+span('RecordAllocation','undirec = new StepData_StepReaderData(nbhead,nbrec,nbpar, theStepModel->SourceCodePage());'))
  s=change(s,'  for ( Standard_Integer nr = 1; nr <= nbrec; nr ++) {','  { StepDiagnostic::Span _diag(StepDiagnostic::RecordConversion);\n  for ( Standard_Integer nr = 1; nr <= nbrec; nr ++) {')
  s=change(s,'  aFileDataModel.ErrorHandle(undirec->GlobalCheck());','  }\n  aFileDataModel.ErrorHandle(undirec->GlobalCheck());')
  s=change(s,'  aFileDataModel.ClearRecorder(1);',span('RawRecorderCleanup','aFileDataModel.ClearRecorder(1);'))
  s=change(s,'  readtool.PrepareHeader(theRecogHeader);',span('PrepareHeader','readtool.PrepareHeader(theRecogHeader);'))
  s=change(s,'  readtool.Prepare(theRecogData);',span('PrepareData','readtool.Prepare(theRecogData);'))
  s=change(s,'  readtool.LoadModel(theStepModel);',span('LoadModel','readtool.LoadModel(theStepModel);')+'\n  StepDiagnostic::Counts(nbhead,nbrec,nbpar,theStepModel->NbEntities());\n  { StepDiagnostic::Span _diag(StepDiagnostic::ReaderCleanup);')
  s=change(s,'  undirec.Nullify();','  undirec.Nullify();\n  }')
 elif relative.startswith('StepData/'):
  s=s.replace('stepdat->SetEntityNumbers(optim);',span('SetEntityNumbers','stepdat->SetEntityNumbers(optim);')).replace('      SetEntities();','      '+span('SetEntities','SetEntities();')).replace('    SetEntities();','    '+span('SetEntities','SetEntities();'))
 elif relative.startswith('IFSelect/'):
  s=change(s,'  if (myModel != model)','  StepDiagnostic::Span _diag(StepDiagnostic::SessionSetModel);\n  if (myModel != model)')
  s=change(s,'{\n  if (thelibrary.IsNull()) return IFSelect_RetVoid;\n  if (theprotocol.IsNull()) return IFSelect_RetVoid;\n  Handle(Interface_InterfaceModel) model;\n  IFSelect_ReturnStatus status = IFSelect_RetVoid;\n  try {\n    OCC_CATCH_SIGNALS\n    Standard_Integer stat = thelibrary->ReadStream', '{\n  StepDiagnostic::ReadGuard _read;\n  if (thelibrary.IsNull()) return IFSelect_RetVoid;\n  if (theprotocol.IsNull()) return IFSelect_RetVoid;\n  Handle(Interface_InterfaceModel) model;\n  IFSelect_ReturnStatus status = IFSelect_RetVoid;\n  try {\n    OCC_CATCH_SIGNALS\n    Standard_Integer stat;\n    { StepDiagnostic::Span _diag(StepDiagnostic::LibraryReadStream); stat = thelibrary->ReadStream')
  s=change(s,'(theName, theIStream, model, theprotocol);','(theName, theIStream, model, theprotocol); }')
  s=change(s,'  if (theprotocol.IsNull()) return Standard_False;\n  if (myModel.IsNull()) return Standard_False;','  StepDiagnostic::Span _diag(StepDiagnostic::SessionComputeGraph);\n  if (theprotocol.IsNull()) return Standard_False;\n  if (myModel.IsNull()) return Standard_False;')
  s=change(s,'  thegraph = new Interface_HGraph (myModel,themodelstat);',span('GraphConstruct','thegraph = new Interface_HGraph (myModel,themodelstat);'))
  s=change(s,'  ComputeCheck();\n  thecheckdone = Standard_True;',span('GraphCheck','ComputeCheck();')+'\n  thecheckdone = Standard_True;')
  s=change(s,'    Interface_Category categ(thegtool);','    StepDiagnostic::Span _category(StepDiagnostic::GraphCategories);\n    Interface_Category categ(thegtool);')
 else:
  s=change(s,'  while (num > 0) {','  { StepDiagnostic::Span _diag(StepDiagnostic::EntityLoadLoop);\n  while (num > 0) {')
  s=change(s,'//  ..        Ajout des Reports, silya','  }\n//  ..        Ajout des Reports, silya')
  s=change(s,'//   Conclusion : peut ne rien faire : selon necessite','  StepDiagnostic::Span _end(StepDiagnostic::EndRead);\n//   Conclusion : peut ne rien faire : selon necessite')
 (out/Path(relative).name).write_text(s)
