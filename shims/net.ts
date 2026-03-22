import { Buffer } from 'node:buffer';

declare function ts_net_server_listen(port: any, handler: any): any;
declare function ts_net_socket_connect(host: any, port: any, emitFn: any): any;
declare function ts_net_socket_write(handle: any, data: any): any;
declare function ts_net_socket_end(handle: any): any;
declare function ts_net_socket_destroy(handle: any): any;
declare function ts_net_socket_set_nodelay(handle: any, enable: any): any;
declare function ts_net_socket_set_keepalive(handle: any, enable: any, delay: any): any;
declare function ts_net_is_ip(str: any): number;
declare function ts_socket_set_pull_mode(handle: any): any;
declare function ts_socket_read_chunk(handle: any): any;

class EventEmitter {
  private _listeners: any;
  constructor() { this._listeners = {}; }
  on(event: string, cb: any): this {
    if (!this._listeners[event]) this._listeners[event] = [];
    this._listeners[event].push({ fn: cb, once: false }); return this;
  }
  once(event: string, cb: any): this {
    if (!this._listeners[event]) this._listeners[event] = [];
    this._listeners[event].push({ fn: cb, once: true }); return this;
  }
  off(event: string, cb: any): this { return this.removeListener(event, cb); }
  removeListener(event: string, cb: any): this {
    if (this._listeners[event])
      this._listeners[event] = this._listeners[event].filter((l: any) => l.fn !== cb);
    return this;
  }
  removeAllListeners(event?: string): this {
    if (event) delete this._listeners[event]; else this._listeners = {}; return this;
  }
  emit(event: string, ...args: any[]): boolean {
    const ls = this._listeners[event];
    if (!ls || ls.length === 0) return false;
    const remaining: any[] = [];
    for (const l of ls) { l.fn(...args); if (!l.once) remaining.push(l); }
    this._listeners[event] = remaining; return true;
  }
  listeners(event: string): any[] { return (this._listeners[event] || []).map((l: any) => l.fn); }
}

export class Socket extends EventEmitter {
  remoteAddress: string = ''; remotePort: number = 0;
  localAddress: string = '';  localPort: number = 0;
  private _handle: any = null;
  destroyed: boolean = false; readable: boolean = true; writable: boolean = true;

  constructor(_options?: any) { super(); }

  connect(portOrOptions: any, host?: any, cb?: any): this {
    let port: number; let hostname: string;
    if (typeof portOrOptions === 'object' && portOrOptions !== null) {
      port = portOrOptions.port ?? 5432;
      hostname = portOrOptions.host ?? 'localhost';
      cb = host;
    } else {
      port = portOrOptions as number;
      hostname = (typeof host === 'string') ? host : 'localhost';
    }
    if (typeof cb === 'function') this.once('connect', cb);
    const self = this;
    const emitFn = (event: string, arg?: any) => {
      if (event === 'end') { self.readable = false; }
      else if (event === 'close') { self.destroyed = true; }
      const ls = self._listeners[event];
      if (!ls || ls.length === 0) return;
      const remaining: any[] = [];
      for (const l of ls) {
        if (event === 'data') { l.fn(Buffer._fromRaw(arg)); }
        else if (event === 'error') { l.fn(new Error(String(arg ?? 'socket error'))); }
        else if (event === 'close') { l.fn(arg); }
        else { l.fn(); }
        if (!l.once) remaining.push(l);
      }
      self._listeners[event] = remaining;
    };
    this._handle = ts_net_socket_connect(hostname, port, emitFn);
    this.remoteAddress = hostname; this.remotePort = port;
    return this;
  }

  write(data: any, encoding?: any, cb?: any): boolean {
    if (this._handle === null || this.destroyed) return false;
    ts_net_socket_write(this._handle, data);
    if (typeof encoding === 'function') encoding();
    else if (typeof cb === 'function') cb();
    return true;
  }

  end(data?: any, encoding?: any, cb?: any): this {
    if (data !== undefined && data !== null) this.write(data, encoding);
    if (this._handle !== null) ts_net_socket_end(this._handle);
    this.writable = false; return this;
  }

  destroy(err?: any): this {
    if (this._handle !== null) { ts_net_socket_destroy(this._handle); this._handle = null; }
    this.destroyed = true; if (err) this.emit('error', err); return this;
  }

  setNoDelay(enable?: boolean): this {
    if (this._handle !== null) ts_net_socket_set_nodelay(this._handle, enable !== false); return this;
  }
  setKeepAlive(enable?: boolean, initialDelay?: number): this {
    if (this._handle !== null) ts_net_socket_set_keepalive(this._handle, enable !== false, initialDelay ?? 0);
    return this;
  }
  setTimeout(_t: number, _cb?: any): this { return this; }
  setEncoding(_enc: string): this { return this; }
  ref(): this { return this; } unref(): this { return this; }
  resume(): this { return this; } pause(): this { return this; }

  /** Switch to pull mode: subsequent data is queued internally instead of
   *  being delivered via 'data' event callbacks. Call readBytes() to consume. */
  setPullMode(): void {
    if (this._handle !== null) ts_socket_set_pull_mode(this._handle);
  }

  /** Read the next available data chunk. Returns null on EOF/close. */
  async readBytes(): Promise<Buffer | null> {
    if (this._handle === null || this.destroyed) return null;
    const raw = await ts_socket_read_chunk(this._handle);
    if (raw === null || raw === undefined) return null;
    return Buffer._fromRaw(raw);
  }
}

class Server extends EventEmitter {
  private _handler: any;
  constructor(handler?: any) { super(); this._handler = handler; }
  listen(port: number, _host?: any, cb?: any): this {
    if (typeof _host === 'function') cb = _host;
    if (cb) cb();
    ts_net_server_listen(port, this._handler || (() => {})); return this;
  }
  close(_cb?: any): this { return this; }
  address(): any { return null; }
}

export function createServer(options?: any, handler?: any): Server {
  if (typeof options === 'function') { handler = options; }
  return new Server(handler);
}
export function createConnection(port: any, host?: any, cb?: any): Socket {
  const sock = new Socket(); sock.connect(port, host, cb); return sock;
}
export function connect(port: any, host?: any, cb?: any): Socket { return createConnection(port, host, cb); }
export function isIP(str: string): number { return ts_net_is_ip(str); }
export function isIPv4(str: string): boolean { return ts_net_is_ip(str) === 4; }
export function isIPv6(str: string): boolean { return ts_net_is_ip(str) === 6; }

export { Server };
const net = { createServer, createConnection, connect, isIP, isIPv4, isIPv6, Server, Socket };
export default net;
