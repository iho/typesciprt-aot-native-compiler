// node:util — common utility functions

export function inspect(value: any, _options?: any): string {
  if (value === null) return 'null';
  if (value === undefined) return 'undefined';
  if (typeof value === 'string') return "'" + value + "'";
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  if (typeof value === 'function') return '[Function: ' + (value.name || 'anonymous') + ']';
  try { return JSON.stringify(value, null, 2); } catch (_) { return '[Object]'; }
}

export function format(fmt: any, ...args: any[]): string {
  if (typeof fmt !== 'string') {
    const parts: string[] = [inspect(fmt)];
    for (let i = 0; i < args.length; i++) parts.push(inspect(args[i]));
    return parts.join(' ');
  }
  let i = 0;
  let result = '';
  let j = 0;
  while (j < fmt.length) {
    if (fmt[j] === '%' && j + 1 < fmt.length) {
      const spec = fmt[j + 1];
      if (spec === 's') { result += i < args.length ? String(args[i++]) : '%s'; j += 2; continue; }
      if (spec === 'd' || spec === 'i') { result += i < args.length ? String(Math.trunc(Number(args[i++]))) : ('%' + spec); j += 2; continue; }
      if (spec === 'f') { result += i < args.length ? String(Number(args[i++])) : '%f'; j += 2; continue; }
      if (spec === 'j') { result += i < args.length ? JSON.stringify(args[i++]) : '%j'; j += 2; continue; }
      if (spec === 'o' || spec === 'O') { result += i < args.length ? inspect(args[i++]) : ('%' + spec); j += 2; continue; }
      if (spec === '%') { result += '%'; j += 2; continue; }
    }
    result += fmt[j++];
  }
  while (i < args.length) result += ' ' + inspect(args[i++]);
  return result;
}

export function promisify(fn: any): (...args: any[]) => Promise<any> {
  return function(...args: any[]): Promise<any> {
    return new Promise((resolve: any, reject: any) => {
      fn(...args, (err: any, result: any) => {
        if (err) reject(err);
        else resolve(result);
      });
    });
  };
}

export function callbackify(fn: (...args: any[]) => Promise<any>): (...args: any[]) => void {
  return function(...args: any[]): void {
    const callback = args.pop();
    fn(...args).then(
      (result: any) => callback(null, result),
      (err: any) => callback(err),
    );
  };
}

export function inherits(ctor: any, superCtor: any): void {
  Object.setPrototypeOf(ctor.prototype, superCtor.prototype);
}

export function deprecate(fn: any, msg: string, _code?: string): any {
  let warned = false;
  return function(...args: any[]): any {
    if (!warned) {
      warned = true;
      console.warn('DeprecationWarning:', msg);
    }
    return fn.apply(this, args);
  };
}

export function isDeepStrictEqual(a: any, b: any): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

export function debuglog(_section: string): (...args: any[]) => void {
  return () => {};
}

export class TextEncoder {
  readonly encoding: string = 'utf-8';
  encode(input?: string): any {
    const s = input || '';
    const bytes: number[] = [];
    for (let i = 0; i < s.length; i++) {
      bytes.push(s.charCodeAt(i));
    }
    return bytes;
  }
}

export class TextDecoder {
  readonly encoding: string;
  constructor(encoding?: string) { this.encoding = encoding || 'utf-8'; }
  decode(input?: any): string {
    if (!input) return '';
    if (typeof input === 'string') return input;
    if (input && typeof input.toString === 'function') return input.toString('utf8');
    return String(input);
  }
}

export const types = {
  isPromise(val: any): boolean { return val !== null && typeof val === 'object' && typeof val.then === 'function'; },
  isRegExp(val: any): boolean { return val instanceof RegExp; },
  isArray(val: any): boolean { return Array.isArray(val); },
  isFunction(val: any): boolean { return typeof val === 'function'; },
  isString(val: any): boolean { return typeof val === 'string'; },
  isNumber(val: any): boolean { return typeof val === 'number'; },
  isBoolean(val: any): boolean { return typeof val === 'boolean'; },
  isNull(val: any): boolean { return val === null; },
  isUndefined(val: any): boolean { return val === undefined; },
  isObject(val: any): boolean { return typeof val === 'object' && val !== null; },
  isNullOrUndefined(val: any): boolean { return val === null || val === undefined; },
  isSymbol(val: any): boolean { return typeof val === 'symbol'; },
};

const util = { inspect, format, promisify, callbackify, inherits, deprecate, isDeepStrictEqual, debuglog, TextEncoder, TextDecoder, types };
export default util;
