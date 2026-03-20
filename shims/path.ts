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

const path = { join, resolve, dirname, basename, extname, normalize, isAbsolute, relative, sep, delimiter };
export default path;
