import { crc32, deflateRawSync } from 'node:zlib';

export function zip(entries: { name: string; data: string; size?: number; compress?: boolean }[]) {
  const local: Buffer[] = [], central: Buffer[] = []; let offset = 0;
  for (const entry of entries) {
    const name = Buffer.from(entry.name), data = Buffer.from(entry.data), payload = entry.compress ? deflateRawSync(data) : data;
    const header = Buffer.alloc(30), record = Buffer.alloc(46), method = entry.compress ? 8 : 0;
    header.writeUInt32LE(0x04034b50); header.writeUInt16LE(method, 8); header.writeUInt32LE(crc32(data), 14);
    header.writeUInt32LE(payload.length, 18); header.writeUInt32LE(entry.size ?? data.length, 22); header.writeUInt16LE(name.length, 26);
    record.writeUInt32LE(0x02014b50); record.writeUInt16LE(method, 10); record.writeUInt32LE(crc32(data), 16);
    record.writeUInt32LE(payload.length, 20); record.writeUInt32LE(entry.size ?? data.length, 24); record.writeUInt16LE(name.length, 28); record.writeUInt32LE(offset, 42);
    local.push(header, name, payload); central.push(record, name); offset += header.length + name.length + payload.length;
  }
  const directory = Buffer.concat(central), end = Buffer.alloc(22); end.writeUInt32LE(0x06054b50);
  end.writeUInt16LE(entries.length, 8); end.writeUInt16LE(entries.length, 10); end.writeUInt32LE(directory.length, 12); end.writeUInt32LE(offset, 16);
  return Buffer.concat([...local, directory, end]);
}
