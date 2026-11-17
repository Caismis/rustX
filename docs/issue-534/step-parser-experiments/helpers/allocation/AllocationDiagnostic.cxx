#include "StepDiagnostic.hxx"
#include "AllocationDiagnostic.hxx"
#include <cerrno>
#include <cstdio>
#include <cstdint>
#include <emscripten.h>
namespace StepDiagnostic {
struct AllocationCount {unsigned long long calls,requested,nullReturns;}; static AllocationCount allocations[Count][4]; static unsigned int depth=0;
void ResetAllocations(){for(int i=0;i<Count;i++)for(int j=0;j<4;j++)allocations[i][j]={0,0,0};}
void Allocation(AllocationKind kind,unsigned long long requested,bool nullReturn){if(!Active())return;const int phase=Current();auto& stat=allocations[phase][kind];++stat.calls;stat.requested+=requested;if(nullReturn)++stat.nullReturns;}
int WriteAllocations(char* out,int at,int capacity){at+=std::snprintf(out+at,capacity-at,",\"allocationEntryTraffic\":[");bool comma=false;const char* names[]={"malloc","calloc","realloc","free"};for(int phase=0;phase<Count;phase++)for(int kind=0;kind<4;kind++){auto& v=allocations[phase][kind];if(!v.calls)continue;at+=std::snprintf(out+at,capacity-at,"%s{\"exclusivePhaseIndex\":%d,\"entrypoint\":\"%s\",\"calls\":\"%llu\",\"requestedByteTraffic\":\"%llu\",\"nullReturns\":\"%llu\"}",comma?",":"",phase,names[kind],v.calls,v.requested,v.nullReturns);comma=true;}return at+std::snprintf(out+at,capacity-at,"]");}
}
extern "C" { void* __real_malloc(std::size_t); void* __real_calloc(std::size_t,std::size_t); void* __real_realloc(void*,std::size_t); void __real_free(void*);
void* __wrap_malloc(std::size_t n){const bool outer=StepDiagnostic::depth++==0;void* p=__real_malloc(n);const int error=errno;--StepDiagnostic::depth;if(outer)StepDiagnostic::Allocation(StepDiagnostic::Malloc,static_cast<unsigned long long>(n),p==nullptr);errno=error;return p;}
void* __wrap_calloc(std::size_t n,std::size_t size){const bool outer=StepDiagnostic::depth++==0;void* p=__real_calloc(n,size);const int error=errno;--StepDiagnostic::depth;if(outer)StepDiagnostic::Allocation(StepDiagnostic::Calloc,static_cast<unsigned long long>(n)*static_cast<unsigned long long>(size),p==nullptr);errno=error;return p;}
void* __wrap_realloc(void* old,std::size_t n){const bool outer=StepDiagnostic::depth++==0;void* p=__real_realloc(old,n);const int error=errno;--StepDiagnostic::depth;if(outer)StepDiagnostic::Allocation(StepDiagnostic::Realloc,static_cast<unsigned long long>(n),p==nullptr);errno=error;return p;}
void __wrap_free(void* p){const bool outer=StepDiagnostic::depth++==0;__real_free(p);const int error=errno;--StepDiagnostic::depth;if(outer)StepDiagnostic::Allocation(StepDiagnostic::Free,0,false);errno=error;}
EMSCRIPTEN_KEEPALIVE int step_allocation_selftest(){StepDiagnostic::ReadGuard guard;int flags=0;unsigned char* p=static_cast<unsigned char*>(__wrap_malloc(32));if(!p)return 1;for(int i=0;i<32;i++)p[i]=static_cast<unsigned char>(i);p=static_cast<unsigned char*>(__wrap_realloc(p,64));if(!p)return 2;for(int i=0;i<32;i++)if(p[i]!=i)flags|=4;__wrap_free(p);void* zero=__wrap_calloc(4,8);if(!zero)flags|=8;else for(int i=0;i<32;i++)if(static_cast<unsigned char*>(zero)[i])flags|=16;__wrap_free(zero);errno=0;void* control=__real_calloc(static_cast<std::size_t>(-1),2);const int expectedErrno=errno;errno=0;void* wrapped=__wrap_calloc(static_cast<std::size_t>(-1),2);const int actualErrno=errno;if(control!=nullptr||wrapped!=nullptr||expectedErrno!=actualErrno)flags|=32;return flags;}
}
