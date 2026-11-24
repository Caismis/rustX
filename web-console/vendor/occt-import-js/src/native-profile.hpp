#pragma once

#ifdef EMSCRIPTEN
#include <emscripten/emscripten.h>
#include <emscripten/heap.h>
#include <malloc.h>
#include <algorithm>
#include <array>

// A bounded, opt-in aggregate; ordinary imports do not read clocks/allocator.
enum class NativePhase { InputCopy, FileParse, TransferHealing, BoundingBox, Tessellation, LodPreflight, Export, FirstCoarse, Final, Count };
struct NativeMetric { double milliseconds = 0; size_t calls = 0; size_t committed = 0; size_t liveSampledMaximum = 0; size_t liveSamples = 0; };
struct NativeProfile {
    int tier = 2;
    std::array<std::array<NativeMetric, 3>, static_cast<size_t>(NativePhase::Count)> metrics;
    void Add (NativePhase phase, double duration) {
        NativeMetric& metric = metrics[static_cast<size_t>(phase)][tier];
        metric.milliseconds += duration; ++metric.calls;
        metric.committed = std::max (metric.committed, emscripten_get_heap_size ());
        // mallinfo walks allocator state. Sample only whole-model boundaries,
        // never each shape's bbox/tessellation (at most 11 walks per import).
        if (phase != NativePhase::BoundingBox && phase != NativePhase::Tessellation) {
            ++metric.liveSamples;
            metric.liveSampledMaximum = std::max (metric.liveSampledMaximum, static_cast<size_t>(mallinfo ().uordblks));
        }
    }
};
inline NativeProfile*& CurrentNativeProfile () { static NativeProfile* profile = nullptr; return profile; }
class NativeSpan {
public:
    explicit NativeSpan (NativePhase phase) : profile (CurrentNativeProfile ()), phase (phase), start (profile ? emscripten_get_now () : 0) {}
    ~NativeSpan () { if (profile) profile->Add (phase, emscripten_get_now () - start); }
private:
    NativeProfile* profile;
    NativePhase phase;
    double start;
};
#else
enum class NativePhase { InputCopy, FileParse, TransferHealing, BoundingBox, Tessellation, LodPreflight, Export, FirstCoarse, Final, Count };
class NativeSpan { public: explicit NativeSpan (NativePhase) {} };
#endif
