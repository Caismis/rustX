declare module 'occt-import-js' {
  interface ImportOptions {
    linearUnit: 'millimeter' | 'centimeter' | 'meter' | 'inch' | 'foot';
    linearDeflectionType: 'bounding_box_ratio' | 'absolute_value';
    linearDeflection: number;
    angularDeflection: number;
    onInputCopied?: () => void;
    /** Benchmark opt-in. Fixed aggregates include nested spans, not additive wall time. */
    onNativeProfile?: (spans: { phase: string; tier: number; durationMs: number; calls: number; inclusive: boolean; wasmCommittedBytes: number; allocatorLiveSamples: number; allocatorLiveSampledMaximumBytes: number | null }[]) => void;
    /** 0 = final only, 1 = coarse/final, 2 = coarse/medium/final. */
    lodTiers?: 0 | 1 | 2;
    onLod?: (level: number, result: unknown) => void;
  }
  interface Occt {
    ReadStepFile(bytes: Uint8Array, options: ImportOptions): unknown;
    ReadIgesFile(bytes: Uint8Array, options: ImportOptions): unknown;
    ReadBrepFile(bytes: Uint8Array, options: ImportOptions): unknown;
  }
  export default function createOcct(options: {
    instantiateWasm: (imports: WebAssembly.Imports, receive: (instance: WebAssembly.Instance, module: WebAssembly.Module) => void) => WebAssembly.Exports;
  }): Promise<Occt>;
}
