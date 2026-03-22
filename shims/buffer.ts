declare function ts_buffer_from_string(str: string, encoding: string): any;
declare function ts_buffer_from_array(arr: any): any;
declare function ts_buffer_alloc(size: number, fill: number): any;
declare function ts_buffer_alloc_unsafe(size: number): any;
declare function ts_buffer_concat(list: any[], totalLength: number): any;
declare function ts_buffer_to_string(buf: any, encoding: string): string;
declare function ts_buffer_to_string_range(buf: any, encoding: string, start: number, end: number): string;
declare function ts_buffer_length(buf: any): number;
declare function ts_buffer_slice(buf: any, start: number, end: number): any;
declare function ts_buffer_get_byte(buf: any, index: number): number;
declare function ts_buffer_set_byte(buf: any, index: number, value: number): number;
declare function ts_buffer_byte_length(str: any, encoding: string): number;
declare function ts_buffer_copy(src: any, target: any, targetStart: number, sourceStart: number, sourceEnd: any): number;
declare function ts_buffer_write_string(buf: any, str: string, offset: number, length: any, encoding: string): number;
declare function ts_buffer_read_u8(buf: any, offset: number): number;
declare function ts_buffer_read_i8(buf: any, offset: number): number;
declare function ts_buffer_read_u16_be(buf: any, offset: number): number;
declare function ts_buffer_read_u16_le(buf: any, offset: number): number;
declare function ts_buffer_read_i16_be(buf: any, offset: number): number;
declare function ts_buffer_read_u32_be(buf: any, offset: number): number;
declare function ts_buffer_read_u32_le(buf: any, offset: number): number;
declare function ts_buffer_read_i32_be(buf: any, offset: number): number;
declare function ts_buffer_read_i32_le(buf: any, offset: number): number;
declare function ts_buffer_read_double_be(buf: any, offset: number): number;
declare function ts_buffer_read_double_le(buf: any, offset: number): number;
declare function ts_buffer_write_u8(buf: any, value: number, offset: number): number;
declare function ts_buffer_write_u16_be(buf: any, value: number, offset: number): number;
declare function ts_buffer_write_u16_le(buf: any, value: number, offset: number): number;
declare function ts_buffer_write_u32_be(buf: any, value: number, offset: number): number;
declare function ts_buffer_write_u32_le(buf: any, value: number, offset: number): number;
declare function ts_buffer_write_i32_be(buf: any, value: number, offset: number): number;
declare function ts_buffer_write_i32_le(buf: any, value: number, offset: number): number;

export class Buffer {
  private _buf: any;
  private constructor(buf: any) { this._buf = buf; }

  static from(data: any, encoding?: string): Buffer {
    if (typeof data === 'string') return new Buffer(ts_buffer_from_string(data, encoding || 'utf8'));
    if (data instanceof Buffer) return new Buffer(ts_buffer_slice(data._buf, 0, ts_buffer_length(data._buf)));
    return new Buffer(ts_buffer_from_array(data));
  }
  static alloc(size: number, fill?: number): Buffer { return new Buffer(ts_buffer_alloc(size, fill ?? 0)); }
  static allocUnsafe(size: number): Buffer { return new Buffer(ts_buffer_alloc_unsafe(size)); }
  static concat(list: Buffer[], totalLength?: number): Buffer {
    return new Buffer(ts_buffer_concat(list.map((b) => b._buf) as any, totalLength ?? 0));
  }
  static byteLength(str: string | Buffer, encoding?: string): number {
    if (str instanceof Buffer) return str.length;
    return ts_buffer_byte_length(str, encoding || 'utf8');
  }
  static isBuffer(obj: any): boolean { return obj instanceof Buffer; }
  /** Wrap a raw TsBuffer handle (from native code) in a Buffer class instance. */
  static _fromRaw(raw: any): Buffer { return new Buffer(raw); }

  toString(encoding?: string, start?: number, end?: number): string {
    if (start !== undefined || end !== undefined) {
      return ts_buffer_to_string_range(this._buf, encoding || 'utf8', start ?? 0, end ?? this.length);
    }
    return ts_buffer_to_string(this._buf, encoding || 'utf8');
  }
  get length(): number { return ts_buffer_length(this._buf); }
  get byteLength(): number { return ts_buffer_length(this._buf); }
  slice(start?: number, end?: number): Buffer { return new Buffer(ts_buffer_slice(this._buf, start ?? 0, end as any)); }
  subarray(start?: number, end?: number): Buffer { return this.slice(start, end); }
  copy(target: Buffer, targetStart?: number, sourceStart?: number, sourceEnd?: number): number {
    return ts_buffer_copy(this._buf, target._buf, targetStart ?? 0, sourceStart ?? 0, sourceEnd as any);
  }
  at(index: number): number { return ts_buffer_get_byte(this._buf, index); }

  readUInt8(offset?: number): number    { return ts_buffer_read_u8(this._buf, offset ?? 0); }
  readInt8(offset?: number): number     { return ts_buffer_read_i8(this._buf, offset ?? 0); }
  readUInt16BE(offset?: number): number { return ts_buffer_read_u16_be(this._buf, offset ?? 0); }
  readUInt16LE(offset?: number): number { return ts_buffer_read_u16_le(this._buf, offset ?? 0); }
  readInt16BE(offset?: number): number  { return ts_buffer_read_i16_be(this._buf, offset ?? 0); }
  readUInt32BE(offset?: number): number { return ts_buffer_read_u32_be(this._buf, offset ?? 0); }
  readUInt32LE(offset?: number): number { return ts_buffer_read_u32_le(this._buf, offset ?? 0); }
  readInt32BE(offset?: number): number  { return ts_buffer_read_i32_be(this._buf, offset ?? 0); }
  readInt32LE(offset?: number): number  { return ts_buffer_read_i32_le(this._buf, offset ?? 0); }
  readDoubleBE(offset?: number): number { return ts_buffer_read_double_be(this._buf, offset ?? 0); }
  readDoubleLE(offset?: number): number { return ts_buffer_read_double_le(this._buf, offset ?? 0); }

  writeUInt8(value: number, offset?: number): number    { return ts_buffer_write_u8(this._buf, value, offset ?? 0); }
  writeInt8(value: number, offset?: number): number     { return ts_buffer_write_u8(this._buf, value & 0xFF, offset ?? 0); }
  writeUInt16BE(value: number, offset?: number): number { return ts_buffer_write_u16_be(this._buf, value, offset ?? 0); }
  writeUInt16LE(value: number, offset?: number): number { return ts_buffer_write_u16_le(this._buf, value, offset ?? 0); }
  writeUInt32BE(value: number, offset?: number): number { return ts_buffer_write_u32_be(this._buf, value, offset ?? 0); }
  writeUInt32LE(value: number, offset?: number): number { return ts_buffer_write_u32_le(this._buf, value, offset ?? 0); }
  writeInt32BE(value: number, offset?: number): number  { return ts_buffer_write_i32_be(this._buf, value, offset ?? 0); }
  writeInt32LE(value: number, offset?: number): number  { return ts_buffer_write_i32_le(this._buf, value, offset ?? 0); }
  write(string: string, offset?: number, length?: any, encoding?: string): number {
    return ts_buffer_write_string(this._buf, string, offset ?? 0, length, encoding || 'utf8');
  }
  fill(value: number, start?: number, end?: number): Buffer {
    const s = start ?? 0; const e = end ?? this.length;
    for (let i = s; i < e; i++) ts_buffer_set_byte(this._buf, i, value);
    return this;
  }
  includes(value: number | string | Buffer): boolean { return this.indexOf(value) !== -1; }
  indexOf(value: number | string | Buffer, byteOffset?: number): number {
    const start = byteOffset ?? 0;
    if (typeof value === 'number') {
      for (let i = start; i < this.length; i++) if (ts_buffer_read_u8(this._buf, i) === value) return i;
    }
    return -1;
  }
  get _handle(): any { return this._buf; }
}
export default Buffer;
