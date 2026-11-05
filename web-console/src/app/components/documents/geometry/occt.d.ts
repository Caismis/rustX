declare module 'occt-import-js' {
  interface ImportOptions {
    linearUnit: 'millimeter' | 'centimeter' | 'meter' | 'inch' | 'foot';
    linearDeflectionType: 'bounding_box_ratio' | 'absolute_value';
    linearDeflection: number;
    angularDeflection: number;
    onInputCopied?: () => void;
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
