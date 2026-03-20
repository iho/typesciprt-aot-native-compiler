declare function ts_buffer_from_string(str: string, encoding: string): any;
declare function ts_buffer_from_array(arr: number[]): any;
declare function ts_buffer_alloc(size: number, fill: number): any;
declare function ts_buffer_alloc_unsafe(size: number): any;
declare function ts_buffer_concat(list: any[], totalLength: number): any;
declare function ts_buffer_to_string(buf: any, encoding: string): string;
declare function ts_buffer_length(buf: any): number;
declare function ts_buffer_slice(buf: any, start: number, end: number): any;
declare function ts_buffer_get_byte(buf: any, index: number): number;

export class Buffer {
  private _buf: any;

  private constructor(buf: any) {
    this._buf = buf;
  }

  static from(data: string | number[], encoding?: string): Buffer {
    if (typeof data === "string") {
      return new Buffer(ts_buffer_from_string(data, encoding || "utf8"));
    }
    return new Buffer(ts_buffer_from_array(data as any));
  }

  static alloc(size: number, fill?: number): Buffer {
    return new Buffer(ts_buffer_alloc(size, fill || 0));
  }

  static allocUnsafe(size: number): Buffer {
    return new Buffer(ts_buffer_alloc_unsafe(size));
  }

  static concat(list: Buffer[], totalLength?: number): Buffer {
    const bufs = list.map((b) => b._buf);
    return new Buffer(ts_buffer_concat(bufs as any, totalLength || 0));
  }

  toString(encoding?: string): string {
    return ts_buffer_to_string(this._buf, encoding || "utf8");
  }

  get length(): number {
    return ts_buffer_length(this._buf);
  }

  slice(start?: number, end?: number): Buffer {
    return new Buffer(ts_buffer_slice(this._buf, start || 0, end as any));
  }

  at(index: number): number {
    return ts_buffer_get_byte(this._buf, index);
  }
}

export default Buffer;
