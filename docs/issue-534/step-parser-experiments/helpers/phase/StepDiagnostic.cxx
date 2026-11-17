#include "StepDiagnostic.hxx"
#include <emscripten.h>
#include <cstdio>
namespace StepDiagnostic {
static bool active=false; static double elapsed[Count]; static int calls[Count]; static int headers,records,parameters,entities;
static const char* names[]={"read-stream","library-read-stream","step-file-read","scanner-parser","record-allocation","record-conversion","raw-recorder-cleanup","prepare-header","prepare-data","set-entity-numbers","set-entities","load-model","entity-load-loop","end-read","reader-cleanup","session-set-model","session-compute-graph","graph-construct","graph-check","graph-categories"};
double Now(){return emscripten_get_now();} bool Active(){return active;}
void Add(Phase phase,double ms){elapsed[phase]+=ms;calls[phase]++;}
void Counts(int h,int r,int p,int e){if(!active)return;headers=h;records=r;parameters=p;if(e>=0)entities=e;}
ReadGuard::ReadGuard(){for(int i=0;i<Count;i++){elapsed[i]=0;calls[i]=0;}headers=records=parameters=entities=0;active=true;start=Now();}
ReadGuard::~ReadGuard(){Add(ReadStream,Now()-start);active=false;char out[8192];int at=std::snprintf(out,sizeof out,"{\"headers\":%d,\"records\":%d,\"parameters\":%d,\"entities\":%d,\"spans\":[",headers,records,parameters,entities);bool comma=false;for(int i=0;i<Count;i++)if(calls[i]){at+=std::snprintf(out+at,sizeof(out)-at,"%s{\"phase\":\"%s\",\"durationMs\":%.6f,\"calls\":%d}",comma?",":"",names[i],elapsed[i],calls[i]);comma=true;}std::snprintf(out+at,sizeof(out)-at,"]}");EM_ASM({console.error('STEP_DIAGNOSTIC '+UTF8ToString($0));},out);}
}
