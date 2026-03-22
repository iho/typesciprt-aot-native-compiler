// Minimal PostgreSQL wire protocol v3 client.
// Supports: trust auth, MD5 password auth, simple query protocol, connection pooling.
// Uses node:net + node:buffer — works with the AOT native compiler.

import { Socket } from 'node:net';
import { Buffer } from 'node:buffer';

// ── Message builders ──────────────────────────────────────────────────────────

function buildStartupMessage(user: string, database: string): Buffer {
  // StartupMessage has no type byte; length field includes itself (4 bytes).
  const params = ['user', user, 'database', database, 'application_name', 'ts-bench'];
  let size = 4 + 4 + 1; // length field + protocol version + trailing null
  for (const p of params) size += p.length + 1; // each string is null-terminated
  const buf = Buffer.alloc(size);
  buf.writeUInt32BE(size, 0);   // total length (includes this 4-byte field)
  buf.writeUInt32BE(196608, 4); // protocol version 3.0
  let o = 8;
  for (const p of params) {
    buf.write(p, o);
    o += p.length;
    buf.writeUInt8(0, o++);
  }
  buf.writeUInt8(0, o); // trailing null — end of params list
  return buf;
}

function buildPasswordMessage(password: string): Buffer {
  // 'p' + Int32(len) + String(password\0)
  const size = 1 + 4 + password.length + 1;
  const buf = Buffer.alloc(size);
  buf.writeUInt8(0x70, 0); // 'p'
  buf.writeUInt32BE(4 + password.length + 1, 1);
  buf.write(password, 5);
  buf.writeUInt8(0, 5 + password.length);
  return buf;
}

function buildSimpleQuery(sql: string): Buffer {
  // 'Q' + Int32(len) + String(sql\0)
  const size = 1 + 4 + sql.length + 1;
  const buf = Buffer.alloc(size);
  buf.writeUInt8(0x51, 0); // 'Q'
  buf.writeUInt32BE(4 + sql.length + 1, 1); // length includes itself
  buf.write(sql, 5);
  buf.writeUInt8(0, 5 + sql.length);
  return buf;
}

function buildTerminate(): Buffer {
  // 'X' + Int32(4)
  const buf = Buffer.alloc(5);
  buf.writeUInt8(0x58, 0); // 'X'
  buf.writeUInt32BE(4, 1);
  return buf;
}

// ── Message parser ────────────────────────────────────────────────────────────

interface PgMsg { type: number; payload: Buffer }

// Try to extract one complete backend message from `buf`.
// Returns [message, remaining_buf] or null if buffer is incomplete.
function parseNextMsg(buf: Buffer): [PgMsg, Buffer] | null {
  if (buf.length < 5) return null;
  const type = buf.readUInt8(0);
  const len = buf.readUInt32BE(1); // includes its own 4 bytes, excludes type byte
  const total = 1 + len;
  if (buf.length < total) return null;
  const payload = buf.slice(5, total);
  const rest = buf.slice(total);
  return [{ type, payload }, rest];
}

// Read a null-terminated C string from buf starting at offset.
// Returns [string, offset_after_null].
function readCString(buf: Buffer, offset: number): [string, number] {
  let end = offset;
  while (end < buf.length && buf.readUInt8(end) !== 0) end++;
  return [buf.toString('utf8', offset, end), end + 1];
}

// ── QueryResult ───────────────────────────────────────────────────────────────

export type Row = Record<string, string | null>;
export interface QueryResult { rows: Row[]; rowCount: number; command: string }

// ── Client ────────────────────────────────────────────────────────────────────

export class Client {
  private _sock: Socket;
  private _user: string;
  private _password: string;
  private _database: string;
  private _host: string;
  private _port: number;
  private _accum: Buffer;
  private _msgs: PgMsg[];

  constructor(cfg: any) {
    const c = cfg || {};
    this._user = c.user || 'postgres';
    this._password = c.password || '';
    this._database = c.database || c.user || 'postgres';
    this._host = c.host || 'localhost';
    this._port = c.port || 5432;
    this._sock = new Socket();
    this._accum = Buffer.alloc(0);
    this._msgs = [];
  }

  // Pull data from the socket into the parse buffer until at least one
  // complete message is available.  No JS lock transitions needed — the
  // data_queue Condvar wakes us up directly via the resolved Promise.
  private async _pullData(): Promise<void> {
    const chunk = await this._sock.readBytes();
    if (chunk === null) throw new Error('socket closed');
    const totalLen = this._accum.length + chunk.length;
    this._accum = Buffer.concat([this._accum, chunk], totalLen);
    let parsed: [PgMsg, Buffer] | null;
    while ((parsed = parseNextMsg(this._accum)) !== null) {
      this._msgs.push(parsed[0]);
      this._accum = parsed[1];
    }
  }

  private async _nextMsg(): Promise<PgMsg> {
    while (this._msgs.length === 0) {
      await this._pullData();
    }
    return this._msgs.shift()!;
  }

  async connect(): Promise<void> {
    const self = this;
    await new Promise<void>((resolve, reject) => {
      self._sock.connect({ host: self._host, port: self._port }, () => resolve());
      self._sock.once('error', (e: Error) => reject(e));
    });

    // Switch to pull mode before sending anything: all subsequent data arrives
    // via readBytes() with no JS lock transitions for delivery.
    this._sock.setPullMode();

    // Send startup message (no type byte prefix)
    this._sock.write(buildStartupMessage(this._user, this._database));

    // Process startup messages until ReadyForQuery ('Z')
    while (true) {
      const msg = await this._nextMsg();
      const t = msg.type;

      if (t === 0x52) { // 'R' — Authentication
        const authType = msg.payload.readUInt32BE(0);
        if (authType === 0) {
          // AuthenticationOk — trust auth, nothing to send
          continue;
        }
        if (authType === 3) {
          // AuthenticationCleartextPassword
          this._sock.write(buildPasswordMessage(this._password));
          continue;
        }
        throw new Error('Auth type ' + authType + ' not supported. Use trust or cleartext password.');
      }

      if (t === 0x53) continue; // 'S' — ParameterStatus (ignore)
      if (t === 0x4b) continue; // 'K' — BackendKeyData (ignore)

      if (t === 0x5a) break; // 'Z' — ReadyForQuery → connected

      if (t === 0x45) { // 'E' — ErrorResponse
        throw new Error(this._parseError(msg.payload));
      }
    }
  }

  async query(sql: string): Promise<QueryResult> {
    this._sock.write(buildSimpleQuery(sql));

    const cols: string[] = [];
    const rows: Row[] = [];
    let command = '';

    while (true) {
      const msg = await this._nextMsg();
      const t = msg.type;

      if (t === 0x54) { // 'T' — RowDescription
        const ncols = msg.payload.readUInt16BE(0);
        let o = 2;
        for (let i = 0; i < ncols; i++) {
          const [name, next] = readCString(msg.payload, o);
          cols.push(name);
          // Skip: tableOID(4) + attrNum(2) + typeOID(4) + typeSize(2) + typeMod(4) + format(2) = 18 bytes
          o = next + 18;
        }
        continue;
      }

      if (t === 0x44) { // 'D' — DataRow
        const ncols = msg.payload.readUInt16BE(0);
        const row: Row = {};
        let o = 2;
        for (let i = 0; i < ncols; i++) {
          const valLen = msg.payload.readInt32BE(o); o += 4;
          if (valLen === -1) {
            row[cols[i]] = null;
          } else {
            row[cols[i]] = msg.payload.toString('utf8', o, o + valLen);
            o += valLen;
          }
        }
        rows.push(row);
        continue;
      }

      if (t === 0x43) { // 'C' — CommandComplete
        const [tag] = readCString(msg.payload, 0);
        const parts = tag.split(' ');
        command = parts[0];
        continue;
      }

      if (t === 0x49) continue; // 'I' — EmptyQueryResponse

      if (t === 0x4e) continue; // 'N' — NoticeResponse (ignore)

      if (t === 0x5a) break; // 'Z' — ReadyForQuery → query complete

      if (t === 0x45) { // 'E' — ErrorResponse
        // Drain until ReadyForQuery
        while (true) {
          const next = await this._nextMsg();
          if (next.type === 0x5a) break;
        }
        throw new Error(this._parseError(msg.payload));
      }
    }

    return { rows, rowCount: rows.length, command };
  }

  end(): void {
    this._sock.write(buildTerminate());
    this._sock.end();
  }

  private _parseError(payload: Buffer): string {
    let o = 0;
    while (o < payload.length) {
      const field = payload.readUInt8(o++);
      if (field === 0) break;
      const [val, next] = readCString(payload, o);
      if (field === 0x4d) return val; // 'M' = Message field
      o = next;
    }
    return 'PostgreSQL error';
  }
}

// ── Pool ──────────────────────────────────────────────────────────────────────

export class Pool {
  private _cfg: any;
  private _size: number;
  private _clients: Client[];
  private _idle: Client[];
  private _waiters: ((c: Client) => void)[];

  constructor(cfg: any) {
    this._cfg = cfg || {};
    this._size = this._cfg.max || 10;
    this._clients = [];
    this._idle = [];
    this._waiters = [];
  }

  async connect(): Promise<void> {
    const connects: Promise<void>[] = [];
    for (let i = 0; i < this._size; i++) {
      const c = new Client(this._cfg);
      this._clients.push(c);
      const self = this;
      connects.push(c.connect().then(() => { self._idle.push(c); }));
    }
    await Promise.all(connects);
  }

  async query(sql: string): Promise<QueryResult> {
    const client = await this._acquire();
    const result = await client.query(sql);
    this._release(client);
    return result;
  }

  private _acquire(): Promise<Client> {
    if (this._idle.length > 0) {
      return Promise.resolve(this._idle.pop()!);
    }
    const self = this;
    return new Promise((resolve) => {
      self._waiters.push(resolve);
    });
  }

  private _release(client: Client): void {
    if (this._waiters.length > 0) {
      const waiter = this._waiters.pop()!;
      waiter(client);
    } else {
      this._idle.push(client);
    }
  }

  async end(): Promise<void> {
    for (const c of this._clients) c.end();
  }
}
