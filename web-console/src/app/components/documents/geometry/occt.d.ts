declare module 'occt-import-js' {
  interface Occt {
    ReadStepFile(bytes: Uint8Array, options: Record<string, string | number>): unknown;
    ReadIgesFile(bytes: Uint8Array, options: Record<string, string | number>): unknown;
    ReadBrepFile(bytes: Uint8Array, options: Record<string, string | number>): unknown;
  }
  export default function createOcct(options: {
    instantiateWasm: (imports: WebAssembly.Imports, receive: (instance: WebAssembly.Instance, module: WebAssembly.Module) => void) => WebAssembly.Exports;
  }): Promise<Occt>;
}
