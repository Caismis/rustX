#ifndef STEP_DIAGNOSTIC_HXX
#define STEP_DIAGNOSTIC_HXX
namespace StepDiagnostic {
enum Phase { ReadStream, LibraryReadStream, FileRead, ScannerParser, RecordAllocation, RecordConversion, RawRecorderCleanup, PrepareHeader, PrepareData, SetEntityNumbers, SetEntities, LoadModel, EntityLoadLoop, EndRead, ReaderCleanup, SessionSetModel, SessionComputeGraph, GraphConstruct, GraphCheck, GraphCategories, Count };
void RecordArguments(int n); void Add(Phase phase, double ms); double Now(); bool Active(); void Counts(int headers,int records,int parameters,int entities=-1);
struct Span { Phase phase; double start; bool active; Span(Phase value):phase(value),start(Active()?Now():0),active(Active()){} ~Span(){if(active)Add(phase,Now()-start);} };
struct ReadGuard { double start; ReadGuard(); ~ReadGuard(); };
}
#endif
