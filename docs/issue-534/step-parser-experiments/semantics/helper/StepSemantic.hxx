#ifndef STEP_SEMANTIC_HXX
#define STEP_SEMANTIC_HXX
namespace StepSemantic { void ScannerFlags(bool bad,bool fail,bool eof); void Record(int nr,const char* id,const char* type,int hasArgs);void Argument(int type,const char* value);void EndRecord();void Entity(int nr,const char* type); }
#endif
