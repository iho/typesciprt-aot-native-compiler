declare function ts_zlib_deflate_sync(data: any): any;
declare function ts_zlib_inflate_sync(data: any): any;
declare function ts_zlib_gzip_sync(data: any): any;
declare function ts_zlib_gunzip_sync(data: any): any;
declare function ts_zlib_deflate_async(data: any): Promise<any>;
declare function ts_zlib_inflate_async(data: any): Promise<any>;
declare function ts_zlib_gzip_async(data: any): Promise<any>;
declare function ts_zlib_gunzip_async(data: any): Promise<any>;

export function deflateSync(data: any, _options?: any): any {
  return ts_zlib_deflate_sync(data);
}
export function inflateSync(data: any, _options?: any): any {
  return ts_zlib_inflate_sync(data);
}
export function gzipSync(data: any, _options?: any): any {
  return ts_zlib_gzip_sync(data);
}
export function gunzipSync(data: any, _options?: any): any {
  return ts_zlib_gunzip_sync(data);
}

export function deflate(data: any, _options: any, callback?: (err: any, result: any) => void): void {
  const cb = typeof _options === 'function' ? _options : callback;
  ts_zlib_deflate_async(data).then((result: any) => { if (cb) cb(null, result); });
}
export function inflate(data: any, _options: any, callback?: (err: any, result: any) => void): void {
  const cb = typeof _options === 'function' ? _options : callback;
  ts_zlib_inflate_async(data).then((result: any) => { if (cb) cb(null, result); });
}
export function gzip(data: any, _options: any, callback?: (err: any, result: any) => void): void {
  const cb = typeof _options === 'function' ? _options : callback;
  ts_zlib_gzip_async(data).then((result: any) => { if (cb) cb(null, result); });
}
export function gunzip(data: any, _options: any, callback?: (err: any, result: any) => void): void {
  const cb = typeof _options === 'function' ? _options : callback;
  ts_zlib_gunzip_async(data).then((result: any) => { if (cb) cb(null, result); });
}

const zlib = { deflateSync, inflateSync, gzipSync, gunzipSync, deflate, inflate, gzip, gunzip };
export default zlib;
