from pathlib import Path
import difflib
source=Path('/tmp/pr556-occt-build/occt/src/StepFile/StepFile_ReadData.cxx');old=source.read_text();s=old
s=s.replace('Record() :myNext(NULL), myFirst(NULL), myIdent(NULL), myType(NULL) {}','Record() :myNext(NULL), myFirst(NULL), myLast(NULL), myIdent(NULL), myType(NULL) {}')
s=s.replace('  Argument* myFirst; //!< First argument in the record','  Argument* myFirst; //!< First argument in the record\n  Argument* myLast;  //!< Last argument in the record')
import re
s,n=re.subn(r'(?m)^(\s*)(myCurRec|aSubRec|aRecord)->myFirst = NULL;$',lambda m:m.group(0)+'\n'+m[1]+m[2]+'->myLast = NULL;',s);assert n==5,n
before='''  if (myCurRec->myFirst == NULL)
  {
    myCurRec->myFirst = aNewArg;
  }
  else
  {
    Argument* aNextArg = myCurRec->myFirst;
    while (aNextArg->myNext != NULL)
      aNextArg = aNextArg->myNext;
    aNextArg->myNext = aNewArg;
  }
'''
after='''  if (myCurRec->myFirst == NULL)
  {
    myCurRec->myFirst = aNewArg;
    myCurRec->myLast = aNewArg;
  }
  else if (myCurRec->myLast == NULL)
  {
    myCurRec->myFirst->myNext = aNewArg;
    myCurRec->myLast = aNewArg;
  }
  else
  {
    Argument* aNextArg = myCurRec->myLast;
    aNextArg->myNext = aNewArg;
    myCurRec->myLast = aNewArg;
  }
'''
assert s.count(before)==1;s=s.replace(before,after)
before='''  Argument* aCurrArg = myCurRec->myFirst;
  while (aCurrArg->myNext != NULL)
    aCurrArg = aCurrArg->myNext;
''';assert s.count(before)==1;s=s.replace(before,'  Argument* aCurrArg = myCurRec->myLast;\n')
root=Path('/tmp/pr556-step-tail-only');(root/'src/StepFile_ReadData.cxx').write_text(s);(root/'tail-only.patch').write_text(''.join(difflib.unified_diff(old.splitlines(True),s.splitlines(True),fromfile='pinned/StepFile_ReadData.cxx',tofile='candidate/StepFile_ReadData.cxx')))
