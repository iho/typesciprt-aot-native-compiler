declare function ts_exec_sync(cmd: string): any;
declare function ts_spawn_sync(cmd: string, args: any, options: any): any;
declare function ts_exec_async(cmd: string): Promise<any>;

export function execSync(cmd: string, _options?: any): string {
  const result = ts_exec_sync(cmd);
  if (result.error) throw new Error('Command failed: ' + result.error);
  return result.stdout || '';
}

export function exec(cmd: string, callback?: (error: any, stdout: string, stderr: string) => void): any {
  if (callback) {
    ts_exec_async(cmd).then((result: any) => {
      if (result.status !== 0) {
        callback(new Error('Command failed with status ' + result.status), result.stdout || '', result.stderr || '');
      } else {
        callback(null, result.stdout || '', result.stderr || '');
      }
    });
  }
  // Return a minimal child-process-like object
  return { pid: 0 };
}

export function spawnSync(cmd: string, args?: string[], options?: any): any {
  return ts_spawn_sync(cmd, args || [], options || {});
}

export function spawn(cmd: string, args?: string[], _options?: any): any {
  // Return a minimal EventEmitter-like object; actual execution is not async-streamed.
  // For simple use cases, run synchronously and emit events.
  const result = ts_spawn_sync(cmd, args || [], {});
  const listeners: any = {};
  const obj = {
    pid: 0,
    stdout: { on: (_evt: string, _cb: any) => {} },
    stderr: { on: (_evt: string, _cb: any) => {} },
    on: (event: string, cb: any) => { listeners[event] = cb; return obj; },
    once: (event: string, cb: any) => { listeners[event] = cb; return obj; },
  };
  // Defer emitting 'close' until next microtask
  if (listeners['close']) listeners['close'](result.status);
  return obj;
}

const childProcess = { execSync, exec, spawnSync, spawn };
export default childProcess;
