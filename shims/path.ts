declare function ts_path_join(parts: any): string;
declare function ts_path_resolve(parts: any): string;
declare function ts_path_dirname(p: string): string;
declare function ts_path_basename(p: string, ext: string): string;
declare function ts_path_extname(p: string): string;
declare function ts_path_normalize(p: string): string;
declare function ts_path_is_absolute(p: string): boolean;
declare function ts_path_relative(from: string, to: string): string;

export function join(...parts: string[]): string { return ts_path_join(parts as any); }
export function resolve(...parts: string[]): string { return ts_path_resolve(parts as any); }
export function dirname(p: string): string { return ts_path_dirname(p); }
export function basename(p: string, ext?: string): string { return ts_path_basename(p, ext || ""); }
export function extname(p: string): string { return ts_path_extname(p); }
export function normalize(p: string): string { return ts_path_normalize(p); }
export function isAbsolute(p: string): boolean { return ts_path_is_absolute(p); }
export function relative(from: string, to: string): string { return ts_path_relative(from, to); }

export const sep = "/";
export const delimiter = ":";

export function parse(p: string): { root: string; dir: string; base: string; ext: string; name: string } {
  const root = p.startsWith('/') ? '/' : '';
  const dir = ts_path_dirname(p);
  const base = ts_path_basename(p, '');
  const ext = ts_path_extname(p);
  const name = ext ? base.substring(0, base.length - ext.length) : base;
  return { root, dir, base, ext, name };
}

export function format(obj: { root?: string; dir?: string; base?: string; ext?: string; name?: string }): string {
  if (obj.base) {
    return obj.dir ? ts_path_join([obj.dir, obj.base] as any) : obj.base;
  }
  const base = (obj.name || '') + (obj.ext || '');
  return obj.dir ? ts_path_join([obj.dir, base] as any) : base;
}

const path = { join, resolve, dirname, basename, extname, normalize, isAbsolute, relative, sep, delimiter, parse, format };
export default path;
