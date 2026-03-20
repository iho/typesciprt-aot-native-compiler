// node:worker_threads — basic stubs for the most common patterns.
// Full worker thread support would require a separate process/thread model.
// These stubs allow code that checks isMainThread to compile and run.

export const isMainThread: boolean = true;
export const threadId: number = 0;
export const workerData: any = null;
export const parentPort: any = null;
export const resourceLimits: any = {};

export class Worker {
  private _script: string;
  private _options: any;
  private _listeners: any;
  threadId: number;

  constructor(filename: string, options?: any) {
    this._script = filename;
    this._options = options || {};
    this._listeners = {};
    this.threadId = 1;
  }

  on(event: string, cb: any): this { this._listeners[event] = cb; return this; }
  once(event: string, cb: any): this { this._listeners[event] = cb; return this; }
  off(event: string, _cb?: any): this { delete this._listeners[event]; return this; }
  postMessage(_value: any): void {}
  terminate(): Promise<number> { return Promise.resolve(0); }
}

export class MessageChannel {
  port1: MessagePort;
  port2: MessagePort;
  constructor() {
    this.port1 = new MessagePort();
    this.port2 = new MessagePort();
  }
}

export class MessagePort {
  on(_evt: string, _cb: any): this { return this; }
  once(_evt: string, _cb: any): this { return this; }
  off(_evt: string, _cb?: any): this { return this; }
  postMessage(_value: any): void {}
  start(): void {}
  close(): void {}
}

export function receiveMessageOnPort(_port: MessagePort): any { return undefined; }
export function markAsUntransferable(_object: object): void {}

const workerThreads = {
  isMainThread, threadId, workerData, parentPort, resourceLimits,
  Worker, MessageChannel, MessagePort, receiveMessageOnPort, markAsUntransferable,
};
export default workerThreads;
