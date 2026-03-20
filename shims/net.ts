declare function ts_net_server_listen(port: any, handler: any): any;
declare function ts_net_connect(port: any, host: any, cb: any): any;

class Server {
  private _port: number;
  private _handler: any;
  constructor(handler?: any) { this._handler = handler; this._port = 0; }
  listen(port: number, _host?: string, cb?: any): this {
    this._port = port;
    if (cb) cb();
    ts_net_server_listen(port, this._handler || (() => {}));
    return this;
  }
  close(): this { return this; }
  on(_evt: string, _cb: any): this { return this; }
}

class Socket {
  remoteAddress: string;
  remotePort: number;
  localAddress: string;
  localPort: number;
  constructor() { this.remoteAddress = ''; this.remotePort = 0; this.localAddress = ''; this.localPort = 0; }
  write(_data: any): boolean { return true; }
  end(): this { return this; }
  destroy(): this { return this; }
  on(_evt: string, _cb: any): this { return this; }
  once(_evt: string, _cb: any): this { return this; }
}

export function createServer(handler?: any): Server { return new Server(handler); }
export function createConnection(port: number, host?: string, cb?: any): Socket {
  const sock = new Socket();
  ts_net_connect(port, host || 'localhost', cb || (() => {}));
  return sock;
}
export function connect(port: number, host?: string, cb?: any): Socket {
  return createConnection(port, host, cb);
}
export { Server, Socket };
const net = { createServer, createConnection, connect, Server, Socket };
export default net;
