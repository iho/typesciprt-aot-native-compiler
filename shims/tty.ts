export function isatty(fd: number): boolean {
  // In compiled native binaries, treat fd 0/1/2 as not a TTY by default.
  // This is conservative and correct for non-interactive use.
  return false;
}

export class ReadStream {
  fd: number;
  isRaw: boolean;
  isTTY: boolean;
  constructor(fd: number) { this.fd = fd; this.isRaw = false; this.isTTY = false; }
  setRawMode(_mode: boolean): this { return this; }
  on(_evt: string, _cb: any): this { return this; }
  once(_evt: string, _cb: any): this { return this; }
  resume(): this { return this; }
  pause(): this { return this; }
}

export class WriteStream {
  fd: number;
  isTTY: boolean;
  columns: number;
  rows: number;
  constructor(fd: number) { this.fd = fd; this.isTTY = false; this.columns = 80; this.rows = 24; }
  write(data: string): boolean { return true; }
  on(_evt: string, _cb: any): this { return this; }
  once(_evt: string, _cb: any): this { return this; }
  getColorDepth(): number { return 1; }
  hasColors(_count?: number): boolean { return false; }
}

const tty = { isatty, ReadStream, WriteStream };
export default tty;
