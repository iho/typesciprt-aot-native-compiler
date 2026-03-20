// Node.js `https` module shim.
// HTTPS is handled transparently by reqwest (the underlying HTTP client),
// so this simply re-exports the same interface as `http`.

declare function ts_http_server_listen(port: number, handler: any): void;
declare function ts_fetch(url: any, init: any): any;

class IncomingMessage {
  method: string;
  url: string;
  headers: any;
  statusCode: number;
  body: string;
  private _rawReq: any;

  constructor(rawReq: any) {
    this._rawReq = rawReq;
    this.method = rawReq.method || 'GET';
    this.url = rawReq.url || '/';
    this.headers = rawReq.headers || {};
    this.statusCode = rawReq.statusCode || 200;
    this.body = rawReq.body || '';
  }

  on(event: string, cb: any): this {
    if (event === 'data') {
      const body = this._rawReq.body;
      if (body) cb(body);
    } else if (event === 'end') {
      cb();
    }
    return this;
  }

  once(event: string, cb: any): this { return this.on(event, cb); }
  resume(): this { return this; }
}

class ServerResponse {
  statusCode: number;
  headersSent: boolean;
  private _statusCode: number;
  private _headers: any;
  private _chunks: string[];
  private _rawRes: any;

  constructor(rawRes: any) {
    this._rawRes = rawRes;
    this._statusCode = 200;
    this._headers = {};
    this._chunks = [];
    this.statusCode = 200;
    this.headersSent = false;
  }

  writeHead(statusCode: number, headers?: any): this {
    this._statusCode = statusCode;
    this.statusCode = statusCode;
    if (headers) {
      const keys = Object.keys(headers);
      for (let i = 0; i < keys.length; i++) {
        this._headers[keys[i]] = headers[keys[i]];
      }
    }
    this.headersSent = true;
    return this;
  }

  setHeader(name: string, value: string): this {
    this._headers[name] = value;
    return this;
  }

  getHeader(name: string): any { return this._headers[name]; }

  write(chunk: any): boolean {
    this._chunks.push(typeof chunk === 'string' ? chunk : String(chunk));
    return true;
  }

  end(data?: any): void {
    if (data !== undefined && data !== null) {
      this._chunks.push(typeof data === 'string' ? data : String(data));
    }
    let body = '';
    for (let i = 0; i < this._chunks.length; i++) body = body + this._chunks[i];
    this._rawRes.__status = this._statusCode;
    this._rawRes.__body = body;
    const headerKeys = Object.keys(this._headers);
    for (let i = 0; i < headerKeys.length; i++) {
      const v = this._headers[headerKeys[i]];
      if (v !== undefined) this._rawRes.__headers[headerKeys[i]] = v;
    }
  }

  send(data: any): void {
    if (typeof data === 'object' && data !== null) {
      this.setHeader('Content-Type', 'application/json');
      this.end(JSON.stringify(data));
    } else {
      this.end(data);
    }
  }

  json(data: any): void {
    this.setHeader('Content-Type', 'application/json');
    this.end(JSON.stringify(data));
  }

  on(_event: string, _cb: any): this { return this; }
}

class Server {
  private _handler: any;
  private _port: number;

  constructor(handler: any) {
    this._handler = handler;
    this._port = 0;
  }

  listen(port: number, callback?: any): this {
    this._port = port;
    const handler = this._handler;
    ts_http_server_listen(port, (rawReq: any, rawRes: any) => {
      const req = new IncomingMessage(rawReq);
      const res = new ServerResponse(rawRes);
      handler(req, res);
    });
    if (callback) callback();
    return this;
  }

  close(callback?: any): void {
    if (callback) callback();
  }

  address(): any {
    return { port: this._port, address: '0.0.0.0', family: 'IPv4' };
  }
}

export function createServer(handler: any): Server {
  return new Server(handler);
}

export function get(url: string, callback?: any): any {
  if (callback) {
    ts_fetch(url, undefined).then((resp: any) => {
      const raw = { method: 'GET', url, headers: {}, body: '', statusCode: resp.status };
      callback(new IncomingMessage(raw));
    });
  }
  return {};
}

export function request(options: any, callback?: any): any {
  let url: string;
  if (typeof options === 'string') {
    url = options;
  } else {
    const host = options.host || options.hostname || 'localhost';
    const port = options.port ? ':' + String(options.port) : '';
    const path = options.path || '/';
    const protocol = options.protocol || 'https:';
    url = protocol + '//' + host + port + path;
  }
  if (callback) {
    const method = (typeof options === 'object' && options.method) ? options.method : 'GET';
    ts_fetch(url, { method }).then((resp: any) => {
      const raw = { method, url, headers: {}, body: '', statusCode: resp.status };
      callback(new IncomingMessage(raw));
    });
  }
  return {};
}

const https = { createServer, get, request, IncomingMessage, ServerResponse };
export default https;
