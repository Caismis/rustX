#include "StepDiagnostic.hxx"
#include <emscripten.h>
#include <cstdio>
#include <climits>
namespace StepDiagnostic {
static bool active=false; static double elapsed[Count]; static int calls[Count]; static int headers,records,parameters,entities;
static unsigned long long argumentRecords,arguments,maxArguments,hops,bins[33],binHops[33]; static bool counterOverflow=false;
void RecordArguments(int n){if(!active)return;const unsigned long long length=static_cast<unsigned long long>(n);int bin=n<4?n:3;if(n>=4)for(unsigned long long x=length;x>=4;x>>=1)++bin;const unsigned long long amount=length>1?(length-1)*(length-2)/2:0;/* Promote before multiplication: n <= INT_MAX, product < 2^62. */if(bin>=33 || ULLONG_MAX-hops<amount || ULLONG_MAX-arguments<length || ULLONG_MAX-binHops[bin]<amount){counterOverflow=true;return;}++argumentRecords;arguments+=length;if(length>maxArguments)maxArguments=length;hops+=amount;++bins[bin];binHops[bin]+=amount;}
static const char* names[]={"read-stream","library-read-stream","step-file-read","scanner-parser","record-allocation","record-conversion","raw-recorder-cleanup","prepare-header","prepare-data","set-entity-numbers","set-entities","load-model","entity-load-loop","end-read","reader-cleanup","session-set-model","session-compute-graph","graph-construct","graph-check","graph-categories"};
double Now(){return emscripten_get_now();} bool Active(){return active;}
void Add(Phase phase,double ms){elapsed[phase]+=ms;calls[phase]++;}
void Counts(int h,int r,int p,int e){if(!active)return;headers=h;records=r;parameters=p;if(e>=0)entities=e;}
ReadGuard::ReadGuard(){for(int i=0;i<Count;i++){elapsed[i]=0;calls[i]=0;}headers=records=parameters=entities=0;argumentRecords=arguments=maxArguments=hops=0;counterOverflow=false;for(int i=0;i<33;i++){bins[i]=binHops[i]=0;}active=true;start=Now();}
ReadGuard::~ReadGuard(){Add(ReadStream,Now()-start);active=false;char out[8192];int at=std::snprintf(out,sizeof out,"{\"headers\":%d,\"records\":%d,\"parameters\":%d,\"entities\":%d,\"spans\":[",headers,records,parameters,entities);bool comma=false;for(int i=0;i<Count;i++)if(calls[i]){at+=std::snprintf(out+at,sizeof(out)-at,"%s{\"phase\":\"%s\",\"durationMs\":%.6f,\"calls\":%d}",comma?",":"",names[i],elapsed[i],calls[i]);comma=true;}at+=std::snprintf(out+at,sizeof(out)-at,"],\"argumentLists\":{\"records\":%llu,\"arguments\":%llu,\"maxArguments\":%llu,\"derivedAppendLinkHops\":\"%llu\",\"overflow\":%s,\"bins\":[",argumentRecords,arguments,maxArguments,hops,counterOverflow?"true":"false");comma=false;for(int i=0;i<33;i++)if(bins[i]){const unsigned long long min=i<4?i:1ULL<<(i-2),max=i<4?i:(1ULL<<(i-1))-1;at+=std::snprintf(out+at,sizeof(out)-at,"%s{\"min\":%llu,\"max\":%llu,\"records\":%llu,\"derivedAppendLinkHops\":\"%llu\"}",comma?",":"",min,max,bins[i],binHops[i]);comma=true;}std::snprintf(out+at,sizeof(out)-at,"]}}");EM_ASM({console.error('STEP_DIAGNOSTIC '+UTF8ToString($0));},out);}
}
