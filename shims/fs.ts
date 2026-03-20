declare function ts_fs_read_file_sync(path: string, encoding: string): string;
declare function ts_fs_write_file_sync(path: string, data: string): void;
declare function ts_fs_exists_sync(path: string): boolean;
declare function ts_fs_mkdir_sync(path: string, options: any): void;
declare function ts_fs_readdir_sync(path: string): string[];
declare function ts_fs_stat_sync(path: string): any;
declare function ts_fs_unlink_sync(path: string): void;
declare function ts_fs_rename_sync(oldPath: string, newPath: string): void;
declare function ts_fs_copy_file_sync(src: string, dst: string): void;
declare function ts_fs_rm_sync(path: string, options: any): void;
declare function ts_fs_read_file_async(path: string, encoding: string): Promise<string>;
declare function ts_fs_write_file_async(path: string, data: string): Promise<void>;

export function readFileSync(path: string, encoding?: string | { encoding?: string }): string {
  const enc = typeof encoding === "string" ? encoding : (encoding as any)?.encoding || "utf8";
  return ts_fs_read_file_sync(path, enc);
}
export function writeFileSync(path: string, data: string, _options?: any): void { ts_fs_write_file_sync(path, data); }
export function existsSync(path: string): boolean { return ts_fs_exists_sync(path); }
export function mkdirSync(path: string, options?: any): void { ts_fs_mkdir_sync(path, options as any); }
export function readdirSync(path: string): string[] { return ts_fs_readdir_sync(path); }
export function statSync(path: string): any { return ts_fs_stat_sync(path); }
export function unlinkSync(path: string): void { ts_fs_unlink_sync(path); }
export function renameSync(oldPath: string, newPath: string): void { ts_fs_rename_sync(oldPath, newPath); }
export function copyFileSync(src: string, dst: string): void { ts_fs_copy_file_sync(src, dst); }
export function rmSync(path: string, options?: any): void { ts_fs_rm_sync(path, options as any); }
export function readFile(path: string, encoding?: string): Promise<string> { return ts_fs_read_file_async(path, encoding || "utf8"); }
export function writeFile(path: string, data: string): Promise<void> { return ts_fs_write_file_async(path, data); }

const fs = { readFileSync, writeFileSync, existsSync, mkdirSync, readdirSync, statSync, unlinkSync, renameSync, copyFileSync, rmSync, readFile, writeFile };
export default fs;

export const promises = { readFile, writeFile };
