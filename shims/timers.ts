declare function ts_set_timeout(cb: any, ms: any): any;
declare function ts_set_interval(cb: any, ms: any): any;
declare function ts_clear_timeout(id: any): any;
declare function ts_clear_interval(id: any): any;

export function setTimeout(cb: any, ms?: number, ...args: any[]): any {
  return ts_set_timeout(cb, ms || 0);
}
export function setInterval(cb: any, ms?: number, ...args: any[]): any {
  return ts_set_interval(cb, ms || 0);
}
export function clearTimeout(id: any): void { ts_clear_timeout(id); }
export function clearInterval(id: any): void { ts_clear_interval(id); }
export function setImmediate(cb: any, ...args: any[]): any {
  return ts_set_timeout(cb, 0);
}
export function clearImmediate(id: any): void { ts_clear_timeout(id); }

const timers = { setTimeout, setInterval, clearTimeout, clearInterval, setImmediate, clearImmediate };
export default timers;
