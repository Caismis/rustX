#ifndef ALLOCATION_DIAGNOSTIC_HXX
#define ALLOCATION_DIAGNOSTIC_HXX
#include <cstddef>
namespace StepDiagnostic { enum AllocationKind { Malloc, Calloc, Realloc, Free }; void Allocation(AllocationKind kind, unsigned long long requested, bool nullReturn); void ResetAllocations(); int WriteAllocations(char* out, int at, int capacity); }
#endif
