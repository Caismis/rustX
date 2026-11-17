import { Color, SRGBColorSpace } from 'three';
import { GEOMETRY_LIMITS as L, requireGeometry as check } from './limits';
import { validateModel, type GeometryModel, type GeometryMesh } from './model';

/** Sequential decoder: at most 83 header bytes / 49 record bytes survive a
 * binary chunk. ASCII retains a bounded token/name and decoded attribute pages. */
export class StlStreamDecoder {
  private offset = 0;
  private prefix = new Uint8Array(0);
  private binary?: BinaryStl;
  private ascii?: AsciiStl;
  /** Only large uncolored binary imports retain one Worker-owned source for
   * the existing exact moving-view proof after the first GPU frame. */
  previewSource?: Uint8Array<ArrayBuffer>;
  constructor(private readonly bytes: number) { check(Number.isSafeInteger(bytes) && bytes >= 0 && bytes <= L.sourceBytes, 'limit'); }
  chunk(data: Uint8Array<ArrayBuffer>, offset: number) {
    check(offset === this.offset && data.length <= 2 * 1024 * 1024 && offset + data.length <= this.bytes);
    this.offset += data.length;
    if (!this.binary && !this.ascii) {
      const take = Math.min(84 - this.prefix.length, data.length), header = new Uint8Array(this.prefix.length + take);
      header.set(this.prefix); header.set(data.subarray(0, take), this.prefix.length); this.prefix = header;
      data = data.subarray(take);
      if (header.length < 84 && this.offset < this.bytes) return;
      const count = header.length >= 84 ? new DataView(header.buffer).getUint32(80, true) : 0;
      if (header.length >= 84 && 84 + count * 50 === this.bytes) {
        this.binary = new BinaryStl(header, count);
        if (!this.binary.hasColors && count * 3 > L.preparationVertices) { this.previewSource = new Uint8Array(this.bytes); this.previewSource.set(header); }
      }
      else { this.ascii = new AsciiStl(); this.ascii.chunk(header); }
      this.prefix = new Uint8Array(0);
    }
    if (this.previewSource) this.previewSource.set(data, this.offset - data.length);
    if (this.binary) this.binary.chunk(data); else this.ascii!.chunk(data);
  }
  finish(): GeometryModel {
    check(this.offset === this.bytes);
    const model = this.binary ? this.binary.finish() : this.ascii?.finish(); check(model, 'empty');
    // Producer and receiver both scan the complete model independently.
    validateModel(model); return model;
  }
}

class BinaryStl {
  private positions: Float32Array;
  private normals: Float32Array;
  private colors?: Float32Array;
  private tail = new Uint8Array(0);
  private face = 0;
  private defaultColor = [0, 0, 0];
  private color = new Color();
  get hasColors() { return this.colors !== undefined; }
  constructor(header: Uint8Array, private readonly count: number) {
    check(count > 0, 'empty'); check(count <= L.triangles && count * 3 <= L.vertices, 'limit');
    const view = new DataView(header.buffer, header.byteOffset, header.byteLength);
    let colored = false;
    for (let index = 0; index <= 70; index++) {
      if (view.getUint32(index, false) === 0x434f4c4f && header[index + 4] === 82 && header[index + 5] === 61) {
        colored = true; this.color.setRGB(header[index + 6] / 255, header[index + 7] / 255, header[index + 8] / 255, SRGBColorSpace);
        this.defaultColor = [this.color.r, this.color.g, this.color.b];
      }
    }
    check(count * 9 * 4 * (colored ? 3 : 2) <= L.decodedBytes, 'limit');
    this.positions = new Float32Array(count * 9); this.normals = new Float32Array(count * 9);
    if (colored) this.colors = new Float32Array(count * 9);
  }
  private record(view: DataView, start: number) {
    check(this.face < this.count);
    const base = this.face++ * 9;
    let color = this.defaultColor;
    if (this.colors) {
      const packed = view.getUint16(start + 48, true);
      if (!(packed & 0x8000)) {
        this.color.setRGB((packed & 31) / 31, ((packed >> 5) & 31) / 31, ((packed >> 10) & 31) / 31, SRGBColorSpace);
        color = [this.color.r, this.color.g, this.color.b];
      }
    }
    for (let vertex = 0; vertex < 3; vertex++) for (let axis = 0; axis < 3; axis++) {
      this.positions[base + vertex * 3 + axis] = view.getFloat32(start + 12 + vertex * 12 + axis * 4, true);
      this.normals[base + vertex * 3 + axis] = view.getFloat32(start + axis * 4, true);
      if (this.colors) this.colors[base + vertex * 3 + axis] = color[axis];
    }
  }
  chunk(data: Uint8Array) {
    if (this.tail.length) {
      const take = Math.min(50 - this.tail.length, data.length), record = new Uint8Array(this.tail.length + take);
      record.set(this.tail); record.set(data.subarray(0, take), this.tail.length); data = data.subarray(take);
      if (record.length < 50) { this.tail = record; return; }
      this.record(new DataView(record.buffer), 0); this.tail = new Uint8Array(0);
    }
    const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
    let offset = 0;
    for (; offset + 50 <= data.length; offset += 50) this.record(view, offset);
    this.tail = data.slice(offset);
  }
  finish(): GeometryModel {
    check(!this.tail.length && this.face === this.count);
    return { meshes: [{ name: '', positions: this.positions, normals: this.normals, colors: this.colors, groups: [] }], root: { name: '', meshes: [0], children: [] } };
  }
}

type Page = { positions: Float32Array; normals: Float32Array; used: number };
type Solid = { name: string; pages: Page[]; triangles: number };
const PAGE_TRIANGLES = 8192;
class AsciiStl {
  private decoder = new TextDecoder('utf-8', { fatal: true });
  private token = '';
  private state: 'solid' | 'facet' | 'normal' | 'normal-values' | 'outer' | 'loop' | 'vertex' | 'vertex-values' | 'endloop' | 'endfacet' = 'solid';
  private nameMode?: 'solid' | 'endsolid';
  private name = '';
  private nameStarted = false;
  private nameLength = 0;
  private nameEnd = 0;
  private solids: Solid[] = [];
  private solid?: Solid;
  private numbers: number[] = [];
  private normal: number[] = [];
  private positions: number[] = [];
  private triangles = 0;
  chunk(data: Uint8Array) { this.text(this.decoder.decode(data, { stream: true })); }
  private text(text: string) {
    for (const character of text) {
      check(character !== '\0');
      if (this.nameMode) {
        if (character === '\r' || character === '\n') this.endName();
        else if (this.nameStarted || !/\s/.test(character)) { this.nameStarted = true; this.nameLength += character.length; if (!/\s/.test(character)) this.nameEnd = this.nameLength; if (this.name.length < 256) this.name += character; }
      } else if (/\s/.test(character)) {
        if (this.token) { const token = this.token; this.token = ''; this.accept(token); if (this.nameMode && (character === '\r' || character === '\n')) this.endName(); }
      } else { check(this.token.length < 128); this.token += character; }
    }
  }
  private endName() {
    if (this.nameMode === 'solid') {
      check(this.solids.length < L.parts, 'limit');
      this.solid = { name: this.name.slice(0, Math.min(256, this.nameEnd)), pages: [], triangles: 0 }; this.solids.push(this.solid); this.state = 'facet';
    } else { check(this.solid && this.solid.triangles > 0, 'empty'); this.solid = undefined; this.state = 'solid'; }
    this.nameMode = undefined; this.name = ''; this.nameStarted = false; this.nameLength = this.nameEnd = 0;
  }
  private accept(token: string) {
    const word = token.toLowerCase();
    switch (this.state) {
      case 'solid': check(word === 'solid'); this.nameMode = 'solid'; break;
      case 'facet':
        if (word === 'endsolid') { this.nameMode = 'endsolid'; break; }
        check(word === 'facet'); this.state = 'normal'; break;
      case 'normal': check(word === 'normal'); this.state = 'normal-values'; this.numbers = []; break;
      case 'normal-values': case 'vertex-values': {
        check(/^[+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?$/i.test(token));
        const value = Number(token); check(Number.isFinite(value) && (this.state === 'normal-values' || Math.abs(value) <= L.coordinate)); this.numbers.push(value);
        if (this.numbers.length === 3) {
          if (this.state === 'normal-values') { this.normal = this.numbers; this.positions = []; this.state = 'outer'; }
          else { this.positions.push(...this.numbers); this.state = this.positions.length === 9 ? 'endloop' : 'vertex'; }
        }
        break;
      }
      case 'outer': check(word === 'outer'); this.state = 'loop'; break;
      case 'loop': check(word === 'loop'); this.state = 'vertex'; break;
      case 'vertex': check(word === 'vertex'); this.state = 'vertex-values'; this.numbers = []; break;
      case 'endloop': check(word === 'endloop'); this.state = 'endfacet'; break;
      case 'endfacet': {
        check(word === 'endfacet'); check(++this.triangles <= L.triangles && this.triangles * 3 <= L.vertices && this.triangles * 72 <= L.decodedBytes, 'limit');
        const solid = this.solid!; let page = solid.pages.at(-1);
        if (!page || page.used === page.positions.length) { const capacity = Math.min(PAGE_TRIANGLES, 32 * 2 ** Math.min(8, solid.pages.length)); page = { positions: new Float32Array(capacity * 9), normals: new Float32Array(capacity * 9), used: 0 }; solid.pages.push(page); }
        page.positions.set(this.positions, page.used);
        for (let vertex = 0; vertex < 3; vertex++) page.normals.set(this.normal, page.used + vertex * 3);
        page.used += 9; solid.triangles++; this.state = 'facet'; break;
      }
    }
  }
  finish(): GeometryModel {
    this.text(this.decoder.decode());
    if (this.token) { this.accept(this.token); this.token = ''; }
    if (this.nameMode) this.endName();
    check(this.state === 'solid' && !this.solid); check(this.triangles > 0, 'empty');
    const meshes: GeometryMesh[] = this.solids.map(solid => {
      const positions = new Float32Array(solid.triangles * 9), normals = new Float32Array(solid.triangles * 9); let offset = 0;
      while (solid.pages.length) { const page = solid.pages.shift()!; positions.set(page.positions.subarray(0, page.used), offset); normals.set(page.normals.subarray(0, page.used), offset); offset += page.used; }
      solid.pages = []; return { name: solid.name, positions, normals, groups: [] };
    });
    return { meshes, root: { name: '', meshes: meshes.map((_, index) => index), children: [] } };
  }
}
