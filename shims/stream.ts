import { EventEmitter } from './events';

// ─── Readable ────────────────────────────────────────────────────────────────

export class Readable extends EventEmitter {
  private _buffer: any[];
  private _ended: boolean;
  private _flowing: boolean;
  private _readFn: ((size?: number) => void) | undefined;
  objectMode: boolean;

  constructor(options?: { read?: (size?: number) => void; objectMode?: boolean }) {
    super();
    this._buffer = [];
    this._ended = false;
    this._flowing = false;
    this._readFn = options && options.read ? options.read : undefined;
    this.objectMode = !!(options && options.objectMode);
  }

  push(chunk: any): boolean {
    if (chunk === null) {
      this._ended = true;
      if (this._flowing) {
        this.emit('end', undefined);
      }
      return false;
    }
    this._buffer.push(chunk);
    if (this._flowing) {
      const item = this._buffer.shift();
      this.emit('data', item);
    }
    return true;
  }

  read(size?: number): any {
    if (this._buffer.length === 0) {
      if (this._readFn) {
        this._readFn(size);
      }
      return null;
    }
    const chunk = this._buffer.shift();
    return chunk !== undefined ? chunk : null;
  }

  resume(): this {
    this._flowing = true;
    // Drain buffered chunks
    while (this._buffer.length > 0) {
      const chunk = this._buffer.shift();
      this.emit('data', chunk);
    }
    if (this._ended) {
      this.emit('end', undefined);
    }
    return this;
  }

  pause(): this {
    this._flowing = false;
    return this;
  }

  pipe(destination: Writable): Writable {
    this.on('data', (chunk: any) => {
      destination.write(chunk);
    });
    this.on('end', (_: any) => {
      destination.end();
    });
    this.resume();
    return destination;
  }

  destroy(err?: any): this {
    if (err) {
      this.emit('error', err);
    }
    this._ended = true;
    this._buffer = [];
    return this;
  }

  static from(iterable: any): Readable {
    const r = new Readable();
    if (Array.isArray(iterable)) {
      for (let i = 0; i < iterable.length; i++) {
        r.push(iterable[i]);
      }
    }
    r.push(null);
    return r;
  }
}

// ─── Writable ────────────────────────────────────────────────────────────────

export class Writable extends EventEmitter {
  private _writeFn: ((chunk: any, encoding: string, callback: () => void) => void) | undefined;
  objectMode: boolean;
  private _ended: boolean;

  constructor(options?: {
    write?: (chunk: any, encoding: string, callback: () => void) => void;
    objectMode?: boolean;
  }) {
    super();
    this._writeFn = options && options.write ? options.write : undefined;
    this.objectMode = !!(options && options.objectMode);
    this._ended = false;
  }

  _write(chunk: any, encoding: string, callback: () => void): void {
    if (this._writeFn) {
      this._writeFn(chunk, encoding, callback);
    } else {
      callback();
    }
  }

  write(chunk: any, encoding?: any, callback?: () => void): boolean {
    const enc = typeof encoding === 'string' ? encoding : 'utf8';
    const cb = typeof encoding === 'function' ? encoding : (callback || (() => {}));
    this._write(chunk, enc, cb);
    this.emit('data', chunk);
    return true;
  }

  end(chunk?: any, callback?: () => void): this {
    if (chunk !== undefined && chunk !== null) {
      this.write(chunk);
    }
    this._ended = true;
    this.emit('finish', undefined);
    if (callback) {
      callback();
    }
    return this;
  }

  destroy(err?: any): this {
    if (err) {
      this.emit('error', err);
    }
    this._ended = true;
    return this;
  }
}

// ─── Duplex ──────────────────────────────────────────────────────────────────

export class Duplex extends Readable {
  private _writeFn: ((chunk: any, encoding: string, callback: () => void) => void) | undefined;
  private _writeEnded: boolean;

  constructor(options?: {
    read?: (size?: number) => void;
    write?: (chunk: any, encoding: string, callback: () => void) => void;
    objectMode?: boolean;
  }) {
    super(options as any);
    this._writeFn = options && options.write ? options.write : undefined;
    this._writeEnded = false;
  }

  _write(chunk: any, encoding: string, callback: () => void): void {
    if (this._writeFn) {
      this._writeFn(chunk, encoding, callback);
    } else {
      callback();
    }
  }

  write(chunk: any, encoding?: any, callback?: () => void): boolean {
    const enc = typeof encoding === 'string' ? encoding : 'utf8';
    const cb = typeof encoding === 'function' ? encoding : (callback || (() => {}));
    this._write(chunk, enc, cb);
    return true;
  }

  end(chunk?: any, callback?: () => void): this {
    if (chunk !== undefined && chunk !== null) {
      this.write(chunk);
    }
    this._writeEnded = true;
    this.emit('finish', undefined);
    if (callback) {
      callback();
    }
    return this;
  }
}

// ─── Transform ───────────────────────────────────────────────────────────────

export class Transform extends Duplex {
  private _transformFn: ((chunk: any, encoding: string, callback: (err?: any, data?: any) => void) => void) | undefined;
  private _flushFn: ((callback: (err?: any, data?: any) => void) => void) | undefined;

  constructor(options?: {
    transform?: (chunk: any, encoding: string, callback: (err?: any, data?: any) => void) => void;
    flush?: (callback: (err?: any, data?: any) => void) => void;
    objectMode?: boolean;
  }) {
    super(options as any);
    this._transformFn = options && options.transform ? options.transform : undefined;
    this._flushFn = options && options.flush ? options.flush : undefined;
  }

  _transform(chunk: any, encoding: string, callback: (err?: any, data?: any) => void): void {
    if (this._transformFn) {
      this._transformFn(chunk, encoding, callback);
    } else {
      callback(undefined, chunk);
    }
  }

  _flush(callback: (err?: any, data?: any) => void): void {
    if (this._flushFn) {
      this._flushFn(callback);
    } else {
      callback();
    }
  }

  _write(chunk: any, encoding: string, callback: () => void): void {
    this._transform(chunk, encoding, (err?: any, data?: any) => {
      if (err) {
        this.emit('error', err);
      } else if (data !== undefined && data !== null) {
        this.push(data);
      }
      callback();
    });
  }

  end(chunk?: any, callback?: () => void): this {
    if (chunk !== undefined && chunk !== null) {
      this.write(chunk);
    }
    this._flush((err?: any, data?: any) => {
      if (err) {
        this.emit('error', err);
      } else {
        if (data !== undefined && data !== null) {
          this.push(data);
        }
        this.push(null);
      }
      this.emit('finish', undefined);
      if (callback) {
        callback();
      }
    });
    return this;
  }
}

// ─── PassThrough ─────────────────────────────────────────────────────────────

export class PassThrough extends Transform {
  constructor(options?: { objectMode?: boolean }) {
    super(options as any);
  }

  _transform(chunk: any, _encoding: string, callback: (err?: any, data?: any) => void): void {
    callback(undefined, chunk);
  }
}

// ─── pipeline ────────────────────────────────────────────────────────────────

export function pipeline(...args: any[]): void {
  if (args.length < 2) {
    return;
  }
  const callback = typeof args[args.length - 1] === 'function' ? args[args.length - 1] : null;
  const streams: any[] = callback ? args.slice(0, args.length - 1) : args;

  if (streams.length < 2) {
    if (callback) callback(new Error('pipeline requires at least 2 streams'));
    return;
  }

  let errored = false;

  const onError = (err: any) => {
    if (!errored) {
      errored = true;
      if (callback) callback(err);
    }
  };

  for (let i = 0; i < streams.length; i++) {
    streams[i].on('error', onError);
  }

  for (let i = 0; i < streams.length - 1; i++) {
    const src = streams[i];
    const dest = streams[i + 1];
    if (typeof src.pipe === 'function') {
      src.pipe(dest);
    }
  }

  const last = streams[streams.length - 1];
  last.on('finish', (_: any) => {
    if (!errored && callback) {
      callback(null);
    }
  });
  last.on('end', (_: any) => {
    if (!errored && callback) {
      callback(null);
    }
  });
}

// ─── finished ────────────────────────────────────────────────────────────────

export function finished(stream: any, callback: (err?: any) => void): void {
  stream.on('finish', (_: any) => { callback(); });
  stream.on('end', (_: any) => { callback(); });
  stream.on('error', (err: any) => { callback(err); });
}

export default {
  Readable,
  Writable,
  Duplex,
  Transform,
  PassThrough,
  pipeline,
  finished,
};
